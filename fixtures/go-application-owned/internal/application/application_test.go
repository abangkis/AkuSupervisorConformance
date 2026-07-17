package application

import (
	"io"
	"net"
	"net/http"
	"reflect"
	"sync"
	"testing"
	"time"
)

func TestShutdownIsIdempotentDrainsRequestAndReleasesResources(t *testing.T) {
	application := New("127.0.0.1", 0, nil)
	address, err := application.Start()
	if err != nil {
		t.Fatal(err)
	}
	origin := "http://" + address.String()
	client := &http.Client{Transport: &http.Transport{DisableKeepAlives: true}}
	response, err := client.Get(origin + "/health")
	if err != nil || response.StatusCode != http.StatusOK {
		t.Fatalf("health request failed: status=%v err=%v", responseStatus(response), err)
	}
	_ = response.Body.Close()

	holdDone := make(chan string, 1)
	go func() {
		response, requestErr := client.Get(origin + "/hold?ms=150")
		if requestErr != nil {
			holdDone <- "error: " + requestErr.Error()
			return
		}
		body, _ := io.ReadAll(response.Body)
		_ = response.Body.Close()
		holdDone <- string(body)
	}()
	waitUntil(t, func() bool { return application.Metrics().ActiveRequests == 1 })

	results := make(chan error, 2)
	var callers sync.WaitGroup
	callers.Add(2)
	for caller := 0; caller < 2; caller++ {
		go func() {
			defer callers.Done()
			results <- application.Shutdown("TEST", time.Second)
		}()
	}
	callers.Wait()
	close(results)
	for shutdownErr := range results {
		if shutdownErr != nil {
			t.Fatalf("shutdown failed: %v", shutdownErr)
		}
	}
	if got := <-holdDone; got != "held" {
		t.Fatalf("active request was not drained: %s", got)
	}

	metrics := application.Metrics()
	if metrics.Ready || metrics.ActiveRequests != 0 || metrics.CleanupRuns != 1 || metrics.WorkerActive || metrics.Listening {
		t.Fatalf("unexpected final metrics: %+v", metrics)
	}
	if connection, dialErr := net.DialTimeout("tcp", address.String(), 100*time.Millisecond); dialErr == nil {
		_ = connection.Close()
		t.Fatal("listener remained reachable")
	}

	events := application.Events()
	names := make([]string, 0, len(events))
	for _, event := range events {
		names = append(names, event.Event)
	}
	want := []string{"server_ready", "shutdown_started", "resource_cleanup_completed", "shutdown_completed"}
	if !reflect.DeepEqual(names, want) {
		t.Fatalf("unexpected events: got=%v want=%v", names, want)
	}
}

func waitUntil(t *testing.T, predicate func() bool) {
	t.Helper()
	deadline := time.Now().Add(2 * time.Second)
	for !predicate() {
		if time.Now().After(deadline) {
			t.Fatal("condition was not reached")
		}
		time.Sleep(10 * time.Millisecond)
	}
}

func responseStatus(response *http.Response) any {
	if response == nil {
		return nil
	}
	return response.StatusCode
}
