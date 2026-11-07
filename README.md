# Claude Graph

Claude Graph is a desktop app (Tauri 2 + React Flow) for engineering work that is too big for one agent session: you draw a directed acyclic graph of nodes, each node is one Claude Code task described by a fixed template (goal, acceptance criterion, upstream inputs, deliverables, model, resource tags), and the app runs the graph through the local `claude` CLI you already have installed and logged in, keeping every brief, prompt, event log and result inside the graph's own folder. This README is a placeholder; see [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the design.
