// `chord bun tests/native-input/verify.ts`: bridge validation; no input is posted.
import assert from "node:assert/strict";
import { click, getAppWindows } from "chord";

assert.equal(typeof click, "function");
assert.equal(typeof getAppWindows, "function");
assert.deepEqual(getAppWindows("dev.keychord.tests.nonexistent-app"), []);
assert.throws(() => getAppWindows(""), /bundleId/);
await assert.rejects(click({ windowId: 0, x: 0, y: 0 }), /windowId/);
await assert.rejects(click({ windowId: 123, x: -1, y: 0 }), /nonnegative/);
await assert.rejects(click({ windowId: 123, x: NaN, y: 0 }));
await assert.rejects(
  click({
    windowId: 123,
    x: 0,
    y: 0,
    expectedBounds: {
      x: 0,
      y: 0,
      width: 0,
      height: 100,
    },
  }),
  /expectedBounds/,
);
await assert.rejects(click({ windowId: 123, x: 0, y: 0, clickCount: 0 }), /clickCount/);
await assert.rejects(click({ windowId: 123, x: 0, y: 0, pressDurationMs: 0 }), /pressDurationMs/);
await assert.rejects(click({ windowId: 123, x: 0, y: 0, button: "unknown" }));
console.log("native-input-bridge-ok (no input posted)");
