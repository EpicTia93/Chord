#!/usr/bin/env bun

import { runSnapshot } from "#runtime";

try {
  process.exitCode = await runSnapshot(process.argv.slice(2));
} catch (error) {
  console.error(`[dev-runtime] ${error}`);
  process.exitCode = 1;
}
