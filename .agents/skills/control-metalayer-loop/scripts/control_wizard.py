#!/usr/bin/env python3
"""Typer wizard for control metalayer setup in agent-operated repositories."""

from __future__ import annotations

import subprocess
from enum import Enum
from pathlib import Path
from typing import Dict, Iterable, List, Tuple

try:
    import typer
except ImportError as exc:  # pragma: no cover - import guard
    raise SystemExit(
        "Missing dependency: typer. Install with `python3 -m pip install typer`."
    ) from exc

app = typer.Typer(help="Control metalayer wizard for agentic repository setup.")
primitive_app = typer.Typer(help="Manage control primitives.")
app.add_typer(primitive_app, name="primitive")

SCRIPT_DIR = Path(__file__).resolve().parent
SKILL_DIR = SCRIPT_DIR.parent
TEMPLATE_DIR = SKILL_DIR / "assets" / "templates"
BOOTSTRAP_SCRIPT = SCRIPT_DIR / "bootstrap_control.sh"
AUDIT_SCRIPT = SCRIPT_DIR / "audit_control.sh"

BASELINE_FILES: Tuple[str, ...] = (
    "AGENTS.md",
    "PLANS.md",
    "METALAYER.md",
    "Makefile.control",
    "scripts/audit_control.sh",
    "scripts/control/smoke.sh",
    "scripts/control/check.sh",
    "scripts/control/test.sh",
    "docs/control/ARCHITECTURE.md",
    "docs/control/OBSERVABILITY.md",
    ".github/workflows/control-harness.yml",
)


class Profile(str, Enum):
    baseline = "baseline"
    governed = "governed"
    autonomous = "autonomous"


class Primitive(str, Enum):
    policy = "policy"
    commands = "commands"
    topology = "topology"
    loop = "loop"
    metrics = "metrics"
    hooks = "hooks"
    recovery = "recovery"
    state = "state"
    nightly = "nightly"
    web = "web"
    cli = "cli"


PRIMITIVE_FILES: Dict[Primitive, Tuple[str, ...]] = {
    Primitive.policy: (".control/policy.yaml",),
    Primitive.commands: (".control/commands.yaml",),
    Primitive.topology: (".control/topology.yaml",),
    Primitive.loop: ("docs/control/CONTROL_LOOP.md",),
    Primitive.metrics: ("evals/control-metrics.yaml",),
    Primitive.hooks: (
        "scripts/control/install_hooks.sh",
        ".githooks/pre-commit",
        ".githooks/pre-push",
    ),
    Primitive.recovery: ("scripts/control/recover.sh",),
    Primitive.state: (".control/state.json",),
    Primitive.nightly: (".github/workflows/control-nightly.yml",),
    Primitive.web: (
        "scripts/control/web_e2e.sh",
        ".github/workflows/web-e2e.yml",
        "tests/e2e/web/smoke.spec.ts",
        "playwright.config.ts",
    ),
    Primitive.cli: (
        "scripts/control/cli_e2e.sh",
        ".github/workflows/cli-e2e.yml",
        "tests/e2e/cli/smoke.sh",
    ),
}

GOVERNED_PRIMITIVES: Tuple[Primitive, ...] = (
    Primitive.policy,
    Primitive.commands,
    Primitive.topology,
    Primitive.loop,
    Primitive.metrics,
    Primitive.hooks,
)

AUTONOMOUS_PRIMITIVES: Tuple[Primitive, ...] = (
    *GOVERNED_PRIMITIVES,
    Primitive.recovery,
    Primitive.state,
    Primitive.nightly,
    Primitive.web,
    Primitive.cli,
)


def _resolve_repo(path: Path) -> Path:
    repo = path.expanduser().resolve()
    if not repo.exists() or not repo.is_dir():
        typer.secho(f"error: repo path does not exist: {repo}", fg=typer.colors.RED, err=True)
        raise typer.Exit(code=2)
    return repo


def _run(script: Path, args: List[str]) -> None:
    if not script.exists():
        typer.secho(f"error: script not found: {script}", fg=typer.colors.RED, err=True)
        raise typer.Exit(code=2)
    result = subprocess.run([str(script), *args], check=False)
    if result.returncode != 0:
        raise typer.Exit(code=result.returncode)


def _copy_template(relative_path: str, repo: Path, force: bool) -> str:
    source = TEMPLATE_DIR / relative_path
    target = repo / relative_path

    if not source.exists():
        typer.secho(f"error: missing template: {source}", fg=typer.colors.RED, err=True)
        raise typer.Exit(code=2)

    target.parent.mkdir(parents=True, exist_ok=True)
    if target.exists() and not force:
        return "skip"

    target.write_bytes(source.read_bytes())
    if target.suffix == ".sh" or relative_path.startswith(".githooks/"):
        target.chmod(0o755)
    return "write"


def _activate_hooks(repo: Path) -> None:
    install_script = repo / "scripts" / "control" / "install_hooks.sh"
    if not install_script.exists():
        return
    result = subprocess.run([str(install_script)], cwd=str(repo), check=False)
    if result.returncode != 0:
        typer.secho(
            "  [warn] failed to activate git hooks automatically; run scripts/control/install_hooks.sh manually.",
            fg=typer.colors.YELLOW,
        )


def _apply_primitives(repo: Path, primitives: Iterable[Primitive], force: bool) -> None:
    for primitive in primitives:
        typer.secho(f"\n[{primitive.value}]", fg=typer.colors.CYAN)
        for relative_path in PRIMITIVE_FILES[primitive]:
            state = _copy_template(relative_path, repo, force)
            label = "write" if state == "write" else "skip "
            typer.echo(f"  [{label}] {relative_path}")
        if primitive == Primitive.hooks:
            _activate_hooks(repo)


@app.command()
def init(
    repo_path: Path = typer.Argument(Path("."), help="Target repository path."),
    profile: Profile = typer.Option(Profile.governed, "--profile", "-p", help="Setup profile."),
    force: bool = typer.Option(False, "--force", help="Overwrite existing files."),
) -> None:
    """Initialize control metalayer in a repository."""
    repo = _resolve_repo(repo_path)
    typer.secho(f"Initializing control metalayer in {repo}", fg=typer.colors.GREEN)

    args = [str(repo)]
    if force:
        args.append("--force")
    _run(BOOTSTRAP_SCRIPT, args)

    if profile == Profile.baseline:
        return

    primitives = GOVERNED_PRIMITIVES if profile == Profile.governed else AUTONOMOUS_PRIMITIVES
    _apply_primitives(repo, primitives, force)
    typer.secho("\nInitialization complete.", fg=typer.colors.GREEN)


@app.command()
def audit(
    repo_path: Path = typer.Argument(Path("."), help="Target repository path."),
    strict: bool = typer.Option(False, "--strict", help="Require governed/autonomous primitives."),
) -> None:
    """Run control metalayer audit."""
    repo = _resolve_repo(repo_path)
    args = [str(repo)]
    if strict:
        args.append("--strict")
    _run(AUDIT_SCRIPT, args)


@app.command()
def status(
    repo_path: Path = typer.Argument(Path("."), help="Target repository path."),
) -> None:
    """Show baseline and primitive coverage."""
    repo = _resolve_repo(repo_path)
    typer.secho(f"Control metalayer status for {repo}", fg=typer.colors.GREEN)
    typer.echo()

    baseline_present = sum(1 for rel in BASELINE_FILES if (repo / rel).exists())
    typer.echo(f"baseline: {baseline_present}/{len(BASELINE_FILES)}")
    for rel in BASELINE_FILES:
        marker = "OK " if (repo / rel).exists() else "MISS"
        typer.echo(f"  [{marker}] {rel}")

    typer.echo()
    typer.echo("primitives:")
    for primitive in Primitive:
        files = PRIMITIVE_FILES[primitive]
        present = sum(1 for rel in files if (repo / rel).exists())
        marker = "OK " if present == len(files) else "PARTIAL" if present > 0 else "MISS"
        typer.echo(f"  [{marker}] {primitive.value}: {present}/{len(files)}")


@primitive_app.command("list")
def primitive_list() -> None:
    """List primitive names and files."""
    for primitive in Primitive:
        typer.echo(primitive.value)
        for rel in PRIMITIVE_FILES[primitive]:
            typer.echo(f"  - {rel}")


@primitive_app.command("add")
def primitive_add(
    primitives: List[Primitive] = typer.Argument(..., help="Primitive names to add."),
    repo: Path = typer.Option(Path("."), "--repo", "-r", help="Target repository path."),
    force: bool = typer.Option(False, "--force", help="Overwrite existing files."),
) -> None:
    """Add selected primitives incrementally."""
    repo_path = _resolve_repo(repo)
    _apply_primitives(repo_path, primitives, force)
    typer.secho("\nPrimitive update complete.", fg=typer.colors.GREEN)


if __name__ == "__main__":
    app()
