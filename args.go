package main

type Args struct {
	ConfigFile   string
	RefreshCache bool
	Verbose      bool
	HostName     string
	SSHArgs      []string
}
