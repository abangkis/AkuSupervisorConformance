import http from "node:http";

const DEFAULT_SHUTDOWN_MS = 4_000;

function defaultLogger(record) {
  console.log(JSON.stringify(record));
}

function listen(server, host, port) {
  return new Promise((resolve, reject) => {
    const onError = (error) => {
      server.off("listening", onListening);
      reject(error);
    };
    const onListening = () => {
      server.off("error", onError);
      resolve(server.address());
    };
    server.once("error", onError);
    server.once("listening", onListening);
    server.listen(port, host);
  });
}

function closeServer(server) {
  return new Promise((resolve, reject) => {
    server.close((error) => (error ? reject(error) : resolve()));
  });
}

function wait(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

export function createApplication({
  host = "127.0.0.1",
  port = 0,
  applicationShutdownMs = DEFAULT_SHUTDOWN_MS,
  logger = defaultLogger,
} = {}) {
  let ready = false;
  let activeRequests = 0;
  let cleanupRuns = 0;
  let shutdownPromise;
  let keepAliveTimer = setInterval(() => {}, 1_000);

  const server = http.createServer(async (request, response) => {
    activeRequests += 1;
    try {
      const requestUrl = new URL(request.url ?? "/", `http://${host}`);
      if (requestUrl.pathname === "/health") {
        response.writeHead(ready ? 200 : 503, {
          "content-type": "application/json",
          connection: "close",
        });
        response.end(JSON.stringify({ status: ready ? "ready" : "stopping" }));
        return;
      }

      if (requestUrl.pathname === "/hold") {
        const requestedMs = Number(requestUrl.searchParams.get("ms") ?? 150);
        const boundedMs = Number.isFinite(requestedMs)
          ? Math.min(Math.max(requestedMs, 1), 2_000)
          : 150;
        await wait(boundedMs);
        response.writeHead(200, {
          "content-type": "text/plain",
          connection: "close",
        });
        response.end("held");
        return;
      }

      response.writeHead(200, {
        "content-type": "text/plain",
        connection: "close",
      });
      response.end("ok");
    } finally {
      activeRequests -= 1;
    }
  });

  async function start() {
    const address = await listen(server, host, port);
    ready = true;
    logger({ event: "server_ready", host, port: address.port });
    return address;
  }

  function shutdown(signal) {
    if (shutdownPromise) return shutdownPromise;

    ready = false;
    logger({ event: "shutdown_started", signal });
    const httpClosed = closeServer(server);
    let deadline;

    const graceful = (async () => {
      cleanupRuns += 1;
      if (keepAliveTimer) {
        clearInterval(keepAliveTimer);
        keepAliveTimer = undefined;
      }
      logger({ event: "resource_cleanup_completed", signal, cleanupRuns });
      await httpClosed;
    })();

    const timedOut = new Promise((_, reject) => {
      deadline = setTimeout(() => {
        server.closeAllConnections?.();
        reject(new Error("application shutdown deadline exceeded"));
      }, applicationShutdownMs);
      deadline.unref?.();
    });

    shutdownPromise = Promise.race([graceful, timedOut])
      .then(() => {
        logger({ event: "shutdown_completed", signal, cleanupRuns });
      })
      .catch((error) => {
        logger({
          event: "shutdown_failed",
          signal,
          message: error.message,
          cleanupRuns,
        });
        throw error;
      })
      .finally(() => clearTimeout(deadline));

    return shutdownPromise;
  }

  return {
    server,
    start,
    shutdown,
    metrics() {
      return {
        ready,
        activeRequests,
        cleanupRuns,
        keepAliveActive: Boolean(keepAliveTimer),
        listening: server.listening,
      };
    },
  };
}

