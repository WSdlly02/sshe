package main

import (
	"context"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"strings"
)

func main() {
	exitCode := run(context.Background(), os.Args[1:])
	os.Exit(exitCode)
}

func run(ctx context.Context, argv []string) int {
	args, err := parseArgs(argv)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error: %v\n", err)
		return 1
	}

	configPath, err := resolveConfigPath(args.ConfigFile)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error: %v\n", err)
		return 1
	}

	config, err := readConfigFile(configPath)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error: %v\n", err)
		return 1
	}

	if err := config.Validate(); err != nil {
		fmt.Fprintf(os.Stderr, "Error: invalid config: %v\n", err)
		return 1
	}

	finalConfig, err := config.GetFinalConfigForHost(args.HostName)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error: %v\n", err)
		return 1
	}

	best, err := selectEndpoint(ctx, args, finalConfig)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error: failed to select endpoint: %v\n", err)
		return 1
	}

	if args.Verbose {
		fmt.Fprintf(os.Stderr, "Using config: %s\n", configPath)
		if args.RefreshCache {
			fmt.Fprintln(os.Stderr, "Cache policy: refresh requested, skipping cached entry")
		}
		fmt.Fprintf(
			os.Stderr,
			"Selected endpoint for %q: %s (%d ms, mode: %s, source: %s)\n",
			args.HostName,
			best.Endpoint,
			best.LatencyMS,
			finalConfig.Host.SelectionMode,
			best.Source,
		)
		fmt.Fprintf(os.Stderr, "Cache path: %s\n", finalConfig.Cache.Path)
	}

	sshArgs := buildSSHArgs(finalConfig, best.Endpoint, args.SSHArgs)
	command := exec.CommandContext(ctx, finalConfig.SSHBin, sshArgs...)
	command.Stdin = os.Stdin
	command.Stdout = os.Stdout
	command.Stderr = os.Stderr

	if err := command.Run(); err != nil {
		var exitErr *exec.ExitError
		if errors.As(err, &exitErr) {
			return exitErr.ExitCode()
		}
		fmt.Fprintf(os.Stderr, "Error: failed to execute ssh binary %q: %v\n", finalConfig.SSHBin, err)
		return 1
	}

	return 0
}

func parseArgs(argv []string) (Args, error) {
	var args Args
	hostSet := false
	parsingSSHArgs := false

	for i := 0; i < len(argv); i++ {
		token := argv[i]

		if parsingSSHArgs {
			args.SSHArgs = append(args.SSHArgs, token)
			continue
		}

		if token == "--" {
			parsingSSHArgs = true
			continue
		}

		if token == "-v" || token == "--verbose" {
			args.Verbose = true
			continue
		}
		if token == "--refresh-cache" {
			args.RefreshCache = true
			continue
		}
		if token == "-c" || token == "--config-file" {
			if i+1 >= len(argv) {
				return Args{}, fmt.Errorf("missing value for %s", token)
			}
			i++
			args.ConfigFile = argv[i]
			continue
		}
		if strings.HasPrefix(token, "--config-file=") {
			args.ConfigFile = strings.TrimPrefix(token, "--config-file=")
			if args.ConfigFile == "" {
				return Args{}, fmt.Errorf("missing value for --config-file")
			}
			continue
		}
		if strings.HasPrefix(token, "-c=") {
			args.ConfigFile = strings.TrimPrefix(token, "-c=")
			if args.ConfigFile == "" {
				return Args{}, fmt.Errorf("missing value for -c")
			}
			continue
		}
		if strings.HasPrefix(token, "-") {
			return Args{}, fmt.Errorf("unknown flag: %s", token)
		}

		if !hostSet {
			args.HostName = token
			hostSet = true
			continue
		}

		return Args{}, fmt.Errorf("ssh arguments must be passed after --")
	}

	if !hostSet {
		return Args{}, fmt.Errorf("missing HOST_NAME")
	}

	return args, nil
}

func selectEndpoint(ctx context.Context, args Args, finalConfig FinalConfig) (ProbeResult, error) {
	if args.RefreshCache {
		probed, err := selectBestEndpoint(ctx, finalConfig.Host)
		if err != nil {
			return ProbeResult{}, err
		}
		if err := storeCachedResult(finalConfig.Cache, finalConfig.HostAlias, finalConfig.Host, probed); err != nil {
			fmt.Fprintf(os.Stderr, "Warning: failed to update cache: %v\n", err)
		}
		return probed, nil
	}

	cached, err := loadCachedResult(finalConfig.Cache, finalConfig.HostAlias, finalConfig.Host)
	if err == nil && cached != nil {
		return *cached, nil
	}
	if err != nil {
		fmt.Fprintf(os.Stderr, "Warning: failed to read cache: %v\n", err)
	}

	probed, probeErr := selectBestEndpoint(ctx, finalConfig.Host)
	if probeErr != nil {
		return ProbeResult{}, probeErr
	}
	if err := storeCachedResult(finalConfig.Cache, finalConfig.HostAlias, finalConfig.Host, probed); err != nil {
		fmt.Fprintf(os.Stderr, "Warning: failed to update cache: %v\n", err)
	}

	return probed, nil
}

func buildSSHArgs(config FinalConfig, endpoint string, passthrough []string) []string {
	args := []string{
		"-i", config.Host.IdentityFile,
		"-p", fmt.Sprintf("%d", config.Host.Port),
		fmt.Sprintf("%s@%s", config.Host.User, endpoint),
	}
	args = append(args, passthrough...)
	return args
}
