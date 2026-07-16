import process from "node:process";

import { createApplication } from "./application.mjs";

function option(name, fallback) {
  const index = process.argv.indexOf(name);
  return index >= 0 && process.argv[index + 1] ? process.argv[index + 1] : fallback;
}

const port = Number(option("--port", process.env.PORT ?? "8091"));
const host = option("--host", process.env.HOST ?? "127.0.0.1");
const applicationShutdownMs = Number(
  option("--shutdown-ms", process.env.APPLICATION_SHUTDOWN_MS ?? "4000"),
);

if (!Number.isInteger(port) || port < 1 || port > 65_535) {
  throw new Error(`invalid --port value: ${port}`);
}
if (!Number.isInteger(applicationShutdownMs) || applicationShutdownMs < 1) {
  throw new Error(`invalid --shutdown-ms value: ${applicationShutdownMs}`);
}

const application = createApplication({ host, port, applicationShutdownMs });

function handleSignal(signal) {
  void application
    .shutdown(signal)
    .then(() => {
      process.exitCode = 0;
    })
    .catch(() => {
      process.exitCode = 1;
    });
}

for (const signal of ["SIGBREAK", "SIGINT", "SIGTERM"]) {
  process.once(signal, handleSignal);
}

process.on("uncaughtException", (error) => {
  console.error(
    JSON.stringify({ event: "uncaught_exception", message: error.message }),
  );
  process.exitCode = 1;
});

await application.start();

