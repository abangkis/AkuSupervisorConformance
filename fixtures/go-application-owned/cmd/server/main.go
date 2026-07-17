package main

import (
	"flag"
	"fmt"
	"os"
	"os/signal"
	"syscall"
	"time"

	"aku-supervisor-conformance/go-application-owned/internal/application"
)

func main() {
	host := flag.String("host", "127.0.0.1", "loopback host")
	port := flag.Int("port", 8092, "listener port")
	shutdownMilliseconds := flag.Int("shutdown-ms", 4000, "application shutdown deadline")
	flag.Parse()
	if *port < 1 || *port > 65535 || *shutdownMilliseconds < 1 {
		fmt.Fprintln(os.Stderr, "invalid service arguments")
		os.Exit(2)
	}

	managed := application.New(*host, *port, application.JSONLogger)
	if _, err := managed.Start(); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}

	shutdownSignals := make(chan os.Signal, 1)
	signal.Notify(shutdownSignals, os.Interrupt, syscall.SIGTERM)
	received := <-shutdownSignals
	signal.Stop(shutdownSignals)
	if err := managed.Shutdown(
		application.CanonicalSignal(received),
		time.Duration(*shutdownMilliseconds)*time.Millisecond,
	); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}
