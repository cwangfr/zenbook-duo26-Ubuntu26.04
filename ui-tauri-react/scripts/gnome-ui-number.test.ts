import assert from "node:assert/strict";
import test from "node:test";

import { gnomeUiNumbers } from "../src/lib/gnome-ui-number.ts";

test("duo built-ins match GNOME Settings reversed numbering", () => {
  const numbers = gnomeUiNumbers(["eDP-1", "eDP-2"]);
  assert.equal(numbers.get("eDP-1"), 2);
  assert.equal(numbers.get("eDP-2"), 1);
});

test("external monitors stay after reversed built-ins", () => {
  const numbers = gnomeUiNumbers(["eDP-1", "HDMI-1", "eDP-2"]);
  assert.deepEqual(
    ["eDP-2", "eDP-1", "HDMI-1"].map((connector) => numbers.get(connector)),
    [1, 2, 3],
  );
});
