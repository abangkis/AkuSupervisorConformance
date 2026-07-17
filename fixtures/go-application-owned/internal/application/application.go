package application

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net"
	"net/http"
	"os"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"time"
)

type Event struct {
	Event       string `json:"event"`
	Signal      string `json:"signal,omitempty"`
	CleanupRuns int64  `json:"cleanupRuns,omitempty"`
	Message     string `json:"message,omitempty"`
}

type Metrics struct {
	Ready          bool
	ActiveRequests int64
	CleanupRuns    int64
	WorkerActive   bool
	Listening      bool
}

type Logger func(Event)

type Application struct {
	host string
	port int

	server *http.Server

	ready          atomic.Bool
	activeRequests atomic.Int64
	cleanupRuns    atomic.Int64
	workerActive   atomic.Bool
	listening      atomic.Bool

	workerStop chan struct{}
	workerDone chan struct{}
	serveDone  chan struct{}

	shutdownOnce sync.Once
	shutdownDone chan struct{}
	shutdownMu   sync.Mutex
	shutdownErr  error

	logger   Logger
	eventsMu sync.Mutex
	events   []Event
}

func New(host string, port int, logger Logger) *Application {
	application := &Application{
		host:         host,
		port:         port,
		workerStop:   make(chan struct{}),
		workerDone:   make(chan struct{}),
		serveDone:    make(chan struct{}),
		shutdownDone: make(chan struct{}),
		logger:       logger,
	}
	mux := http.NewServeMux()
	mux.HandleFunc("/health", application.handleHealth)
	mux.HandleFunc("/hold", application.handleHold)
	mux.HandleFunc("/", application.handleDefault)
	application.server = &http.Server{
		Handler:           application.trackRequests(mux),
		ReadHeaderTimeout: 2 * time.Second,
	}
	return application
}

func (application *Application) Start() (net.Addr, error) {
	listener, err := net.Listen("tcp", net.JoinHostPort(application.host, strconv.Itoa(application.port)))
	if err != nil {
		return nil, err
	}
	application.listening.Store(true)
	application.ready.Store(true)
	application.workerActive.Store(true)
	go func() {
		defer close(application.workerDone)
		ticker := time.NewTicker(25 * time.Millisecond)
		defer ticker.Stop()
		for {
			select {
			case <-ticker.C:
			case <-application.workerStop:
				application.workerActive.Store(false)
				return
			}
		}
	}()
	go func() {
		defer close(application.serveDone)
		err := application.server.Serve(listener)
		application.listening.Store(false)
		if err != nil && !errors.Is(err, http.ErrServerClosed) {
			application.record(Event{Event: "serve_failed", Message: err.Error()})
		}
	}()
	application.record(Event{Event: "server_ready"})
	return listener.Addr(), nil
}

func (application *Application) Shutdown(signal string, timeout time.Duration) error {
	application.shutdownOnce.Do(func() {
		go application.shutdown(signal, timeout)
	})
	<-application.shutdownDone
	application.shutdownMu.Lock()
	defer application.shutdownMu.Unlock()
	return application.shutdownErr
}

func (application *Application) shutdown(signal string, timeout time.Duration) {
	defer close(application.shutdownDone)
	application.ready.Store(false)
	application.record(Event{Event: "shutdown_started", Signal: signal})
	close(application.workerStop)

	ctx, cancel := context.WithTimeout(context.Background(), timeout)
	defer cancel()
	err := application.server.Shutdown(ctx)
	<-application.serveDone
	<-application.workerDone
	cleanupRuns := application.cleanupRuns.Add(1)
	application.record(Event{
		Event:       "resource_cleanup_completed",
		Signal:      signal,
		CleanupRuns: cleanupRuns,
	})
	if err != nil {
		application.record(Event{Event: "shutdown_failed", Signal: signal, Message: err.Error()})
	} else {
		application.record(Event{
			Event:       "shutdown_completed",
			Signal:      signal,
			CleanupRuns: cleanupRuns,
		})
	}
	application.shutdownMu.Lock()
	application.shutdownErr = err
	application.shutdownMu.Unlock()
}

func (application *Application) Metrics() Metrics {
	return Metrics{
		Ready:          application.ready.Load(),
		ActiveRequests: application.activeRequests.Load(),
		CleanupRuns:    application.cleanupRuns.Load(),
		WorkerActive:   application.workerActive.Load(),
		Listening:      application.listening.Load(),
	}
}

func (application *Application) Events() []Event {
	application.eventsMu.Lock()
	defer application.eventsMu.Unlock()
	return append([]Event(nil), application.events...)
}

func (application *Application) trackRequests(next http.Handler) http.Handler {
	return http.HandlerFunc(func(response http.ResponseWriter, request *http.Request) {
		application.activeRequests.Add(1)
		defer application.activeRequests.Add(-1)
		next.ServeHTTP(response, request)
	})
}

func (application *Application) handleHealth(response http.ResponseWriter, _ *http.Request) {
	if !application.ready.Load() {
		http.Error(response, "stopping", http.StatusServiceUnavailable)
		return
	}
	response.Header().Set("Content-Type", "application/json")
	_, _ = response.Write([]byte(`{"status":"ready"}`))
}

func (application *Application) handleHold(response http.ResponseWriter, request *http.Request) {
	milliseconds, _ := strconv.Atoi(request.URL.Query().Get("ms"))
	if milliseconds < 1 || milliseconds > 2000 {
		milliseconds = 150
	}
	time.Sleep(time.Duration(milliseconds) * time.Millisecond)
	_, _ = response.Write([]byte("held"))
}

func (application *Application) handleDefault(response http.ResponseWriter, _ *http.Request) {
	_, _ = response.Write([]byte("ok"))
}

func (application *Application) record(event Event) {
	application.eventsMu.Lock()
	application.events = append(application.events, event)
	application.eventsMu.Unlock()
	if application.logger != nil {
		application.logger(event)
	}
}

func JSONLogger(event Event) {
	_ = json.NewEncoder(os.Stdout).Encode(event)
}

func CanonicalSignal(signal os.Signal) string {
	if signal == os.Interrupt {
		return "SIGINT"
	}
	name := strings.ToUpper(signal.String())
	if name == "TERMINATED" || name == "SIGTERM" {
		return "SIGTERM"
	}
	return fmt.Sprintf("%s", signal)
}
