# Python Build & Toolchain Patterns

Pure toolchain reference for Python projects, for profile authors who write a profile's gates and templates. These
patterns apply to any domain.

A project declares its tool in `spec.toml` (`[project] toolchain = "uv" | "poetry" | "pip"`); the profile gives one
gate list per tool, and `check` reports a lockfile that does not belong to the declared tool. Every command below is
given for all three tools.

---

## pyproject.toml

The manifest is `pyproject.toml`, and its `[project]` table (PEP 621) is read by every tool here (poetry from 2.0).
Minimal library with a `src/` layout:

```toml
[project]
name = "my-package"
version = "0.1.0"
description = "One-line package description."
requires-python = ">=3.11"
dependencies = ["attrs>=23"]

[dependency-groups]            # PEP 735: dev-only dependencies
dev = ["pytest>=8", "mypy>=1.11", "ruff>=0.6"]

[build-system]
requires = ["hatchling"]
build-backend = "hatchling.build"
```

The importable package is `src/my_package/` (the distribution name uses `-`, the import name `_`). In `spec.toml`,
`[paths].source` names the package directory: `src/my_package`.

A native extension built from Rust with PyO3 uses maturin as the build backend:

```toml
[build-system]
requires = ["maturin>=1.7,<2"]
build-backend = "maturin"

[tool.maturin]
module-name = "my_package._native"   # the compiled module inside the package
python-source = "src"
```

## Lockfiles and toolchain files

| Tool | Lockfile | Notes |
|---|---|---|
| uv | `uv.lock` | `uv sync` writes it |
| poetry | `poetry.lock` | `poetry lock` writes it |
| pip | none, or `pylock.toml` (PEP 751) | `pip lock` writes `pylock.toml` (pip ≥ 25.1); a `requirements.txt` is not a lockfile |

`.python-version` pins the interpreter for uv and pyenv. Commit the lockfile.

## Commands

| Job | uv | poetry | pip (in an activated venv) |
|---|---|---|---|
| Create the environment and install | `uv sync` | `poetry install` | `python -m venv .venv && . .venv/bin/activate && pip install -e . --group dev` |
| Run a command in it | `uv run <cmd>` | `poetry run <cmd>` | `<cmd>` |
| Tests | `uv run python -m pytest` | `poetry run python -m pytest` | `python -m pytest` |
| Type check | `uv run python -m mypy src` | `poetry run python -m mypy src` | `python -m mypy src` |
| Lint and format check | `uv run python -m ruff check` / `… ruff format --check` | `poetry run python -m ruff check` | `python -m ruff check` |
| Build a wheel | `uv build` | `poetry build` | `python -m build` |
| Native extension, editable | `uv sync` (maturin backend) | `poetry install` | `maturin develop` |

`python -m <tool>` is the neutral form: it runs the tool of the active environment. A gate list for a tool prefixes
it with that tool's runner (`uv run`, `poetry run`); the pip list runs it as is, in the environment the gate script is
started in.

## Tests

pytest discovers `test_*.py` and `*_test.py` files, `test*` functions at module level, and `test*` methods of
`Test*` classes (no `__init__`) and of `unittest.TestCase` subclasses. specdev's discovery follows the same rules; a
requirement's `verified_by` names a test by its bare name, or by `<file>::<Class>::<name>` when the bare name is
not unique. Keep tests under `tests/`.

## Imports and layering

`check` holds each `import` between project modules to the module's `depends_on`. An import inside
`if TYPE_CHECKING:` is for the type checker only and is not a dependency. A third-party import is the manifest's
business, not the spec's.
