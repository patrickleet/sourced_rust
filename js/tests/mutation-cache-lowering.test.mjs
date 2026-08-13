/**
 * Shared mutation cache-lowering vectors (Rust ↔ JS parity).
 *
 * Drives the shipped `lowerMutationCache` implementation against the frozen
 * golden mutation-program-v1 fixture — not a reimplementation in the test.
 */

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

import {
  lowerMutationCache,
  MUTATION_CACHE_VISIBILITY_UNAUTHORIZED,
} from "../dist/replica/mutation-cache.js";

const __dirname = dirname(fileURLToPath(import.meta.url));
const fixturePath = join(
  __dirname,
  "../../tests/fixtures/mutation-program-v1.json",
);

test("golden mutation-program-v1 lowers to complete upsert via shipped JS", () => {
  const program = JSON.parse(readFileSync(fixturePath, "utf8"));
  assert.equal(program.ir_version, 1);
  assert.equal(program.name, "save_todo");
  assert.ok(!("arms" in program));
  assert.ok(!("selector" in program));

  const lowered = lowerMutationCache(program);
  assert.equal(lowered.effects.length, 1);
  assert.equal(lowered.effects[0].kind, "upsert");
  assert.equal(lowered.effects[0].target.model, "Todos");
  assert.ok(lowered.effects[0].fields.includes("todo_id"));
  assert.ok(lowered.effects[0].fields.includes("title"));
});

test("unauthorized visibility fails closed to invalidation", () => {
  const program = JSON.parse(readFileSync(fixturePath, "utf8"));
  const lowered = lowerMutationCache(program, MUTATION_CACHE_VISIBILITY_UNAUTHORIZED);
  assert.equal(lowered.effects[0].kind, "invalidate");
});

test("patch without base never creates a record", () => {
  const program = {
    ir_version: 1,
    operations: [
      {
        kind: "patch",
        target: { model: "Todos", storage: "todos" },
        fields: [
          {
            name: "status",
            assignment: { kind: "set", expression: { kind: "constant" } },
          },
        ],
      },
    ],
  };
  const lowered = lowerMutationCache(program, {
    authorized: true,
    hasBaseRecord: false,
    relationshipCovered: true,
  });
  assert.equal(lowered.effects[0].kind, "invalidate");
});

test("delete lowers to provisional hide", () => {
  const program = {
    ir_version: 1,
    operations: [
      {
        kind: "delete",
        target: { model: "Todos", storage: "todos" },
        fields: [],
      },
    ],
  };
  const lowered = lowerMutationCache(program);
  assert.equal(lowered.effects[0].kind, "delete");
  assert.equal(lowered.effects[0].target.model, "Todos");
});
