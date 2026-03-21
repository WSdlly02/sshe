package main

import (
	"context"
	"fmt"
	"net"
	"os/exec"
	"regexp"
	"strconv"
	"sync"
	"time"
)

type ProbeSource string

const (
	ProbeSourceCache ProbeSource = "cache"
	ProbeSourceProbe ProbeSource = "probe"
)

type ProbeResult struct {
	Endpoint  string
	LatencyMS int64
	Source    ProbeSource
}

func selectBestEndpoint(ctx context.Context, host FinalHostConfig) (ProbeResult, error) {
	timeout := time.Duration(host.ProbeTimeoutMS) * time.Millisecond
	limit := host.ProbeConcurrency
	if limit <= 0 {
		limit = 1
	}

	semaphore := make(chan struct{}, limit)
	results := make(chan probeOutcome, len(host.Endpoints))
	var wg sync.WaitGroup

	for _, endpoint := range host.Endpoints {
		endpoint := endpoint
		wg.Add(1)

		go func() {
			defer wg.Done()

			select {
			case semaphore <- struct{}{}:
			case <-ctx.Done():
				results <- probeOutcome{Err: fmt.Errorf("%s:%d -> %w", endpoint, host.Port, ctx.Err())}
				return
			}
			defer func() { <-semaphore }()

			var latency time.Duration
			var err error
			switch host.SelectionMode {
			case SelectionModeLowestICMPLatency:
				latency, err = probeICMP(ctx, endpoint, timeout)
			default:
				latency, err = probeTCP(ctx, endpoint, host.Port, timeout)
			}
			if err != nil {
				results <- probeOutcome{Err: fmt.Errorf("%s:%d -> %w", endpoint, host.Port, err)}
				return
			}

			results <- probeOutcome{
				Result: ProbeResult{
					Endpoint:  endpoint,
					LatencyMS: latency.Milliseconds(),
					Source:    ProbeSourceProbe,
				},
			}
		}()
	}

	go func() {
		wg.Wait()
		close(results)
	}()

	var best *ProbeResult
	var lastErr error
	for outcome := range results {
		if outcome.Err != nil {
			lastErr = outcome.Err
			continue
		}

		if best == nil || outcome.Result.LatencyMS < best.LatencyMS {
			result := outcome.Result
			best = &result
		}
	}

	if best == nil {
		if lastErr != nil {
			return ProbeResult{}, lastErr
		}
		return ProbeResult{}, fmt.Errorf("no reachable endpoint found")
	}

	return *best, nil
}

type probeOutcome struct {
	Result ProbeResult
	Err    error
}

func probeTCP(ctx context.Context, host string, port uint16, timeout time.Duration) (time.Duration, error) {
	address := fmt.Sprintf("%s:%d", host, port)
	dialer := &net.Dialer{Timeout: timeout}
	start := time.Now()

	conn, err := dialer.DialContext(ctx, "tcp", address)
	if err != nil {
		return 0, fmt.Errorf("connect failed: %w", err)
	}
	_ = conn.Close()

	return time.Since(start), nil
}

func probeICMP(ctx context.Context, host string, timeout time.Duration) (time.Duration, error) {
	timeoutSec := int(timeout / time.Second)
	if timeoutSec < 1 {
		timeoutSec = 1
	}

	commandCtx, cancel := context.WithTimeout(ctx, timeout)
	defer cancel()

	output, err := exec.CommandContext(commandCtx, "ping", "-c", "1", "-W", strconv.Itoa(timeoutSec), host).CombinedOutput()
	if err != nil {
		if commandCtx.Err() == context.DeadlineExceeded {
			return 0, fmt.Errorf("ping timeout")
		}
		return 0, fmt.Errorf("ping failed: %w", err)
	}

	latency, err := parsePingLatency(string(output))
	if err != nil {
		return 0, err
	}

	return latency, nil
}

var pingLatencyPattern = regexp.MustCompile(`time=([0-9]+(?:\.[0-9]+)?)\s*ms`)

func parsePingLatency(output string) (time.Duration, error) {
	matches := pingLatencyPattern.FindStringSubmatch(output)
	if len(matches) != 2 {
		return 0, fmt.Errorf("unable to parse ping latency")
	}

	value, err := strconv.ParseFloat(matches[1], 64)
	if err != nil {
		return 0, fmt.Errorf("unable to parse ping latency: %w", err)
	}

	return time.Duration(value * float64(time.Millisecond)), nil
}
