# Contributing

Thanks for looking. Issues and pull requests are welcome.

## Run the tests

```bash
npm install
npx vitest run                  # frontend
cd src-tauri && cargo test      # backend, including end-to-end runs with the fake CLI
```

CI runs the same two suites plus `npm run build` and `cargo check --all-targets` on
Windows and Ubuntu. Please keep both green.

The real-CLI tests are ignored by default. They use your Claude Code login and cost
about one cent:

```bash
cd src-tauri && cargo test --test real_claude -- --ignored --nocapture
```

To try the app without spending anything, see "Try it without spending anything" in
the [README](README.md#install).

## The IPC contract comes first

The frontend and the backend talk through a fixed set of Tauri commands and events,
described in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) section 6. The types live in
two files that must match field for field:

- `src/types/contract.ts`
- `src-tauri/src/contract.rs`

To change a command, an event or a shared type:

1. Update the contract section of `docs/ARCHITECTURE.md`.
2. Update both contract files in the same commit. Field names are snake_case on both
   sides, with no serde renaming.
3. Update the mock in `src/lib/ipc.mock.ts` so mock mode behaves like the backend,
   including its error codes.
4. Then change the implementations and add tests on both sides.

Graph logic belongs in pure modules (`src/lib/*.ts`, `src-tauri/src/scheduler.rs` and
friends) so it can be tested without a window.

## Commits

Use [Conventional Commits](https://www.conventionalcommits.org/):

```
<type>: <description>
```

Types: `feat`, `fix`, `refactor`, `docs`, `test`, `chore`, `perf`, `ci`.
