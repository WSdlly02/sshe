package main

import (
	"fmt"
	"os"
	"path/filepath"

	"github.com/BurntSushi/toml"
)

type SsheConfig struct {
	Global *GlobalConfig         `toml:"global"`
	Hosts  map[string]HostConfig `toml:"hosts"`
}

type GlobalConfig struct {
	SSHBin           *string        `toml:"ssh_bin"`
	ProbeTimeoutMS   *uint64        `toml:"probe_timeout_ms"`
	ProbeConcurrency *int           `toml:"probe_concurrency"`
	CacheTTLSec      *uint64        `toml:"cache_ttl_sec"`
	CachePath        *string        `toml:"cache_path"`
	SelectionMode    *SelectionMode `toml:"selection_mode"`
}

type HostConfig struct {
	User           string         `toml:"user"`
	Port           uint16         `toml:"port"`
	IdentityFile   string         `toml:"identity_file"`
	ProbeTimeoutMS *uint64        `toml:"probe_timeout_ms"`
	SelectionMode  *SelectionMode `toml:"selection_mode"`
	Endpoints      []string       `toml:"endpoints"`
}

type FinalConfig struct {
	HostAlias string
	SSHBin    string
	Host      FinalHostConfig
	Cache     CacheConfig
}

type CacheConfig struct {
	TTLSeconds uint64
	Path       string
}

type FinalHostConfig struct {
	User             string
	Port             uint16
	IdentityFile     string
	ProbeTimeoutMS   uint64
	ProbeConcurrency int
	SelectionMode    SelectionMode
	Endpoints        []string
}

type SelectionMode string

const (
	SelectionModeLowestICMPLatency SelectionMode = "lowest_icmp_latency"
	SelectionModeLowestTCPLatency  SelectionMode = "lowest_tcp_latency"
)

func readConfigFile(path string) (SsheConfig, error) {
	var config SsheConfig
	if _, err := toml.DecodeFile(path, &config); err != nil {
		return SsheConfig{}, fmt.Errorf("failed to parse TOML config %s: %w", path, err)
	}
	return config, nil
}

func (c *SsheConfig) Validate() error {
	if len(c.Hosts) == 0 {
		return fmt.Errorf("at least one host configuration is required")
	}

	if c.Global != nil {
		if c.Global.ProbeTimeoutMS != nil && *c.Global.ProbeTimeoutMS == 0 {
			return fmt.Errorf("global probe_timeout_ms must be greater than 0")
		}
		if c.Global.ProbeConcurrency != nil && *c.Global.ProbeConcurrency == 0 {
			return fmt.Errorf("global probe_concurrency must be greater than 0")
		}
		if c.Global.CacheTTLSec != nil && *c.Global.CacheTTLSec == 0 {
			return fmt.Errorf("global cache_ttl_sec must be greater than 0")
		}
		if err := validateSelectionMode(c.Global.SelectionMode, true); err != nil {
			return err
		}
	}

	for name, host := range c.Hosts {
		if host.User == "" {
			return fmt.Errorf("host %q has empty user", name)
		}
		if host.Port == 0 {
			return fmt.Errorf("host %q has invalid port 0", name)
		}
		if host.IdentityFile == "" {
			return fmt.Errorf("host %q has empty identity_file", name)
		}
		if len(host.Endpoints) == 0 {
			return fmt.Errorf("host %q must have at least one endpoint", name)
		}
		if host.ProbeTimeoutMS != nil && *host.ProbeTimeoutMS == 0 {
			return fmt.Errorf("host %q probe_timeout_ms must be greater than 0", name)
		}
		if err := validateSelectionMode(host.SelectionMode, true); err != nil {
			return fmt.Errorf("host %q %w", name, err)
		}
	}

	return nil
}

func (c *SsheConfig) GetFinalConfigForHost(name string) (FinalConfig, error) {
	host, ok := c.Hosts[name]
	if !ok {
		return FinalConfig{}, fmt.Errorf("no configuration found for host %q", name)
	}

	sshBin := "ssh"
	probeTimeoutMS := uint64(500)
	probeConcurrency := 4
	cacheTTL := uint64(300)
	cachePath, err := defaultCachePath()
	if err != nil {
		return FinalConfig{}, err
	}
	selectionMode := SelectionModeLowestTCPLatency

	if c.Global != nil {
		if c.Global.SSHBin != nil && *c.Global.SSHBin != "" {
			sshBin = *c.Global.SSHBin
		}
		if c.Global.ProbeTimeoutMS != nil && *c.Global.ProbeTimeoutMS > 0 {
			probeTimeoutMS = *c.Global.ProbeTimeoutMS
		}
		if c.Global.ProbeConcurrency != nil && *c.Global.ProbeConcurrency > 0 {
			probeConcurrency = *c.Global.ProbeConcurrency
		}
		if c.Global.CacheTTLSec != nil && *c.Global.CacheTTLSec > 0 {
			cacheTTL = *c.Global.CacheTTLSec
		}
		if c.Global.CachePath != nil && *c.Global.CachePath != "" {
			cachePath, err = expandTilde(*c.Global.CachePath)
			if err != nil {
				return FinalConfig{}, err
			}
		}
		if c.Global.SelectionMode != nil && *c.Global.SelectionMode != "" {
			selectionMode = *c.Global.SelectionMode
		}
	}

	if host.ProbeTimeoutMS != nil && *host.ProbeTimeoutMS > 0 {
		probeTimeoutMS = *host.ProbeTimeoutMS
	}
	if host.SelectionMode != nil && *host.SelectionMode != "" {
		selectionMode = *host.SelectionMode
	}

	identityFile, err := expandTilde(host.IdentityFile)
	if err != nil {
		return FinalConfig{}, err
	}

	return FinalConfig{
		HostAlias: name,
		SSHBin:    sshBin,
		Host: FinalHostConfig{
			User:             host.User,
			Port:             host.Port,
			IdentityFile:     identityFile,
			ProbeTimeoutMS:   probeTimeoutMS,
			ProbeConcurrency: probeConcurrency,
			SelectionMode:    selectionMode,
			Endpoints:        append([]string(nil), host.Endpoints...),
		},
		Cache: CacheConfig{
			TTLSeconds: cacheTTL,
			Path:       cachePath,
		},
	}, nil
}

func resolveConfigPath(configFile string) (string, error) {
	if configFile != "" {
		info, err := os.Stat(configFile)
		if err != nil {
			return "", fmt.Errorf("config file does not exist or is not a regular file: %s", configFile)
		}
		if !info.Mode().IsRegular() {
			return "", fmt.Errorf("config file does not exist or is not a regular file: %s", configFile)
		}
		return configFile, nil
	}

	home, err := os.UserHomeDir()
	if err != nil {
		return "", fmt.Errorf("HOME is not set, cannot determine default config path")
	}

	candidates := []string{
		filepath.Join(home, ".ssh", "sshe.toml"),
		filepath.Join(home, ".config", "sshe.toml"),
		filepath.Join(home, ".config", "sshe", "config.toml"),
	}

	for _, candidate := range candidates {
		info, err := os.Stat(candidate)
		if err == nil && info.Mode().IsRegular() {
			return candidate, nil
		}
	}

	return "", fmt.Errorf(
		"default config file does not exist: %s %s %s",
		candidates[0],
		candidates[1],
		candidates[2],
	)
}

func defaultCachePath() (string, error) {
	return filepath.Join("/run/user", fmt.Sprintf("%d", os.Getuid()), "sshe", "cache.toml"), nil
}

func expandTilde(path string) (string, error) {
	if path == "" {
		return "", nil
	}

	if path == "~" {
		return os.UserHomeDir()
	}

	if len(path) >= 2 && path[:2] == "~/" {
		home, err := os.UserHomeDir()
		if err != nil {
			return "", fmt.Errorf("HOME is not set")
		}
		return filepath.Join(home, path[2:]), nil
	}

	return path, nil
}

func validateSelectionMode(mode *SelectionMode, allowEmpty bool) error {
	if mode == nil {
		if allowEmpty {
			return nil
		}
		return fmt.Errorf("has invalid selection_mode %q", "")
	}

	if *mode == "" && allowEmpty {
		return nil
	}

	switch *mode {
	case "", SelectionModeLowestICMPLatency, SelectionModeLowestTCPLatency:
		return nil
	default:
		return fmt.Errorf("has invalid selection_mode %q", *mode)
	}
}
