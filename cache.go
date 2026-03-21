package main

import (
	"fmt"
	"os"
	"path/filepath"
	"time"

	"github.com/BurntSushi/toml"
)

type CacheFile struct {
	Entries map[string]CacheEntry `toml:"entries"`
}

type CacheEntry struct {
	Endpoint      string        `toml:"endpoint"`
	LatencyMS     int64         `toml:"latency_ms"`
	ExpiresAtUnix int64         `toml:"expires_at_unix"`
	Port          uint16        `toml:"port"`
	SelectionMode SelectionMode `toml:"selection_mode"`
}

func loadCachedResult(cache CacheConfig, hostAlias string, host FinalHostConfig) (*ProbeResult, error) {
	cacheFile, err := readCacheFile(cache.Path)
	if err != nil {
		return nil, err
	}

	entry, ok := cacheFile.Entries[hostAlias]
	if !ok {
		return nil, nil
	}

	if entry.ExpiresAtUnix <= time.Now().Unix() {
		return nil, nil
	}
	if entry.Port != host.Port || entry.SelectionMode != host.SelectionMode {
		return nil, nil
	}
	if !containsString(host.Endpoints, entry.Endpoint) {
		return nil, nil
	}

	return &ProbeResult{
		Endpoint:  entry.Endpoint,
		LatencyMS: entry.LatencyMS,
		Source:    ProbeSourceCache,
	}, nil
}

func storeCachedResult(cache CacheConfig, hostAlias string, host FinalHostConfig, result ProbeResult) error {
	cacheFile, err := readCacheFile(cache.Path)
	if err != nil {
		return err
	}
	if cacheFile.Entries == nil {
		cacheFile.Entries = make(map[string]CacheEntry)
	}

	cacheFile.Entries[hostAlias] = CacheEntry{
		Endpoint:      result.Endpoint,
		LatencyMS:     result.LatencyMS,
		ExpiresAtUnix: time.Now().Add(time.Duration(cache.TTLSeconds) * time.Second).Unix(),
		Port:          host.Port,
		SelectionMode: host.SelectionMode,
	}

	return writeCacheFile(cache.Path, cacheFile)
}

func readCacheFile(path string) (CacheFile, error) {
	var cacheFile CacheFile

	info, err := os.Stat(path)
	if err != nil {
		if os.IsNotExist(err) {
			return CacheFile{Entries: make(map[string]CacheEntry)}, nil
		}
		return CacheFile{}, fmt.Errorf("failed to read cache file %s: %w", path, err)
	}
	if !info.Mode().IsRegular() {
		return CacheFile{}, fmt.Errorf("cache file is not a regular file: %s", path)
	}

	if _, err := toml.DecodeFile(path, &cacheFile); err != nil {
		return CacheFile{}, fmt.Errorf("failed to parse cache file %s: %w", path, err)
	}
	if cacheFile.Entries == nil {
		cacheFile.Entries = make(map[string]CacheEntry)
	}

	return cacheFile, nil
}

func writeCacheFile(path string, cacheFile CacheFile) error {
	parent := filepath.Dir(path)
	if err := os.MkdirAll(parent, 0o755); err != nil {
		return fmt.Errorf("failed to create cache directory %s: %w", parent, err)
	}

	file, err := os.Create(path)
	if err != nil {
		return fmt.Errorf("failed to write cache file %s: %w", path, err)
	}
	defer file.Close()

	if err := toml.NewEncoder(file).Encode(cacheFile); err != nil {
		return fmt.Errorf("failed to serialize cache file %s: %w", path, err)
	}

	return nil
}

func containsString(values []string, target string) bool {
	for _, value := range values {
		if value == target {
			return true
		}
	}
	return false
}
