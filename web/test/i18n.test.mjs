import assert from "node:assert/strict";
import { test } from "node:test";
import { createTranslator } from "../src/i18n.ts";

test("partial translations fall back to Japanese", () => {
  const t = createTranslator({ "documentation.loading": "Loading…" });
  assert.equal(t("documentation.loading"), "Loading…");
  assert.equal(t("documentation.notFound"), "見つかりません");
});

test("interpolation does not reinterpret user values or dollar replacements", () => {
  const t = createTranslator({ "landing.help": "{command}: {command}" });
  assert.equal(t("landing.help", { command: "$&{command}<b>" }), "$&{command}<b>: $&{command}<b>");
});

test("unfilled snippet placeholders survive and inherited properties are not parameters", () => {
  const t = createTranslator({ "landing.help": "{command} {toString}" });
  assert.equal(t("landing.help"), "{command} {toString}");
});
