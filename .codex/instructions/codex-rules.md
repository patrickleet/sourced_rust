# Codex Rules (Harmony)

## GitKB First

- For non-trivial work, create or update a GitKB document before coding.
- Use `git-kb` for tasks, context, and traceability.

## Code Intelligence

- Do not use grep to find callers/definitions.
- Use GitKB code tools (`git-kb code callers`, `kb_callers`, etc.).

## Commit Discipline

- Always scope `git-kb commit` with pathspecs.
- Include task slugs in commit messages (e.g., `[[tasks/<task-slug>]]`).
