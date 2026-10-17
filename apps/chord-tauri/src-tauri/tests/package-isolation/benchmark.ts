// Optimized, repeatable A/B measurements using the production package supervisor.
import { mkdir } from "node:fs/promises";

const repo = Bun.fileURLToPath(new URL("../../../../../", import.meta.url));
const manifest = Bun.fileURLToPath(new URL("../../Cargo.toml", import.meta.url));
const args = process.argv.slice(2);
function option(name: string, fallback: string): string {
  const index = args.indexOf(name);
  if (index < 0) return fallback;
  const value = args[index + 1];
  if (!value || value.startsWith("--")) throw new Error(name + " needs a value");
  return value;
}
const runs = Number(option("--runs", "3"));
if (!Number.isInteger(runs) || runs < 1 || runs > 20) throw new Error("--runs must be 1..20");
const target = Bun.fileURLToPath(
  new URL(
    (process.env.CARGO_TARGET_DIR ?? "apps/chord-tauri/src-tauri/target").replace(/\/$/, "") + "/",
    Bun.pathToFileURL(repo + "/"),
  ),
);
const output = Bun.fileURLToPath(
  new URL(
    option(
      "--output",
      target + "/package-worker-benchmarks/" + new Date().toISOString().replaceAll(":", "-"),
    ).replace(/\/$/, "") + "/",
    Bun.pathToFileURL(repo + "/"),
  ),
);
await mkdir(output, { recursive: true });

async function command(argv: string[], capture = false): Promise<string> {
  const child = Bun.spawn(argv, {
    cwd: repo,
    env: { ...process.env, CARGO_TARGET_DIR: target, CARGO_INCREMENTAL: "0" },
    stdout: capture ? "pipe" : "inherit",
    stderr: "inherit",
    timeout: 1_800_000,
  });
  const text = capture ? await new Response(child.stdout).text() : "";
  const status = await child.exited;
  if (status !== 0) throw new Error(argv[0] + " failed (" + status + ")");
  return text;
}

await command(["cargo", "build", "--release", "--manifest-path", manifest, "--bin", "chord"]);
const artifacts = await command(
  [
    "cargo",
    "test",
    "--release",
    "--manifest-path",
    manifest,
    "--lib",
    "--no-run",
    "--message-format=json",
  ],
  true,
);
const artifact = artifacts
  .split("\n")
  .filter(Boolean)
  .map((line) => JSON.parse(line))
  .find(
    (item) =>
      item.reason === "compiler-artifact" &&
      item.profile.test &&
      item.executable &&
      item.target.name === "chords_lib",
  );
if (!artifact) throw new Error("Cargo did not return the library test executable");

const reports: any[] = [];
for (let run = 1; run <= runs; run++) {
  console.log("\nPaired benchmark run " + run + "/" + runs);
  const path = output + "/run-" + run + ".json";
  const child = Bun.spawn(
    [
      artifact.executable,
      "bun_js::package_worker::benchmarks::package_worker_benchmarks",
      "--exact",
      "--ignored",
      "--nocapture",
      "--test-threads=1",
    ],
    {
      cwd: repo,
      env: {
        ...process.env,
        CHORD_PACKAGE_TEST_HOST: target + "/release/chord",
        CHORD_BENCH_OUTPUT: path,
        CHORD_BENCH_SAMPLES: option("--samples", "1000"),
        CHORD_BENCH_WORKERS: option("--workers", "4"),
      },
      stdout: "inherit",
      stderr: "inherit",
      timeout: 300_000,
    },
  );
  if ((await child.exited) !== 0) throw new Error("Benchmark run " + run + " failed");
  const report = await Bun.file(path).json();
  if (report.debug_assertions) throw new Error("Refusing to report an unoptimized debug run");
  reports.push(report);
}

const median = (values: number[]) => {
  const sorted = [...values].sort((a, b) => a - b);
  const middle = Math.floor(sorted.length / 2);
  return sorted.length % 2 ? sorted[middle] : (sorted[middle - 1] + sorted[middle]) / 2;
};
const cases = reports[0].cases.map((item: any, index: number) => {
  const metric = (path: "in_process" | "worker", field: string) =>
    median(reports.map((r) => r.cases[index][path][field]));
  const inProcess = metric("in_process", "p50_us");
  const worker = metric("worker", "p50_us");
  return {
    name: item.name,
    in_process_p50_us: inProcess,
    worker_p50_us: worker,
    delta_p50_us: worker - inProcess,
    in_process_p95_us: metric("in_process", "p95_us"),
    worker_p95_us: metric("worker", "p95_us"),
    worker_over_in_process: worker / inProcess,
    in_process_ops_s: metric("in_process", "operations_per_second"),
    worker_ops_s: metric("worker", "operations_per_second"),
  };
});
const system = await command(["system_profiler", "SPHardwareDataType", "-json"], true);
// Record model/chip/memory, excluding serial numbers and hardware UUIDs.
const hardware = JSON.parse(system).SPHardwareDataType[0];
const summary = {
  schema_version: 1,
  measured_at: new Date().toISOString(),
  runs,
  samples: reports[0].samples,
  warmup: reports[0].warmup,
  workers: reports[0].workers,
  hardware: {
    model: hardware.machine_model,
    chip: hardware.chip_type,
    memory: hardware.physical_memory,
  },
  os: (await command(["sw_vers", "-productVersion"], true)).trim(),
  rust: (await command(["rustc", "--version"], true)).trim(),
  runner_bun: Bun.version,
  embedded_bun: reports[0].embedded_bun_version,
  cases,
  cold: {
    in_process_us: median(reports.map((r) => r.cold.in_process_first_vm_import_factory_us)),
    worker_us: median(reports.map((r) => r.cold.worker_process_vm_import_factory.p50_us)),
  },
  idle: reports.map((r) => r.idle),
  codec: reports[0].codec.map((c: any, i: number) => ({
    payload_bytes: c.payload_bytes,
    p50_us: median(reports.map((r) => r.codec[i].roundtrip.p50_us)),
  })),
};
await Bun.write(output + "/summary.json", JSON.stringify(summary, null, 2) + "\n");
console.log("\nMedians across " + runs + " fresh harness processes (microseconds):");
console.table(
  cases.map((c: any) => ({
    case: c.name,
    in_process: c.in_process_p50_us.toFixed(2),
    worker: c.worker_p50_us.toFixed(2),
    extra: c.delta_p50_us.toFixed(2),
    worker_p95: c.worker_p95_us.toFixed(2),
  })),
);
console.log("Results: " + output + "/summary.json");
