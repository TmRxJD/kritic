# Example: a repo with a kritic plugin

`.kritic/plugins/todo-budget.toml` declares a plugin; `tools/todo-budget.mjs` implements it.
Copy both into your repo, then ask Claude to run `plugin_list(base_dir=<repo>)` and
`plugin_run(base_dir=<repo>, name="todo-budget", args={max_new: 0})`.

Try it by hand, without kritic:

```bash
echo '{"args":{"max_new":0},"base_dir":".","out_dir":"."}' | node tools/todo-budget.mjs
```

See [docs/plugins.md](../../docs/plugins.md) for the full contract, including optional Kev questions.
