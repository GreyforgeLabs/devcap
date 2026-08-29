"""Command line interface for devcap."""

from __future__ import annotations

import argparse
import math
import sys

from .formatters import FORMATTERS
from .profile_loader import (
    MAX_PROFILE_DEPTH,
    list_builtin_profiles,
    load_builtin_profile,
    load_custom_profile,
)
from .scanner import (
    COMMAND_TIMEOUT_SECONDS,
    MAX_COMMAND_TIMEOUT_SECONDS,
    MAX_SCAN_WORKERS,
    redact_scan,
    scan_tools,
)


def _finite_positive_timeout(value: str) -> float:
    try:
        parsed = float(value)
    except ValueError as exc:
        raise argparse.ArgumentTypeError("must be a number") from exc
    if not math.isfinite(parsed) or parsed <= 0 or parsed > MAX_COMMAND_TIMEOUT_SECONDS:
        raise argparse.ArgumentTypeError(
            f"must be greater than 0 and at most {MAX_COMMAND_TIMEOUT_SECONDS:g}"
        )
    return parsed


def _bounded_positive_integer(value: str, *, maximum: int) -> int:
    try:
        parsed = int(value)
    except ValueError as exc:
        raise argparse.ArgumentTypeError("must be an integer") from exc
    if parsed < 1 or parsed > maximum:
        raise argparse.ArgumentTypeError(f"must be between 1 and {maximum}")
    return parsed


def main(argv: list[str] | None = None) -> int:
    """Run the CLI."""
    parser = _build_parser()
    args = parser.parse_args(argv)

    if not args.command:
        parser.print_help()
        return 0

    if args.command == "list-profiles":
        return _cmd_list_profiles()

    if args.command in ("scan", "check"):
        return _cmd_scan(args)

    parser.print_help()
    return 0


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="devcap",
        description="Scan your development environment for installed tools and capabilities.",
    )
    subparsers = parser.add_subparsers(dest="command")

    # scan
    scan_parser = subparsers.add_parser("scan", help="Scan for installed tools")
    _add_scan_arguments(scan_parser)

    # check
    check_parser = subparsers.add_parser(
        "check", help="Check that required tools are installed (exit 1 if missing)"
    )
    _add_scan_arguments(check_parser)

    # list-profiles
    subparsers.add_parser("list-profiles", help="List available built-in profiles")

    return parser


def _add_scan_arguments(parser: argparse.ArgumentParser) -> None:
    parser.add_argument(
        "--format",
        choices=("text", "json", "markdown"),
        default="text",
        help="Output format (default: text)",
    )
    profile_group = parser.add_mutually_exclusive_group()
    profile_group.add_argument(
        "--profile",
        help="Built-in profile name (e.g. python-dev, devops, full)",
    )
    profile_group.add_argument(
        "--config",
        help="Path to a custom TOML profile file",
    )
    parser.add_argument(
        "--timeout",
        type=_finite_positive_timeout,
        default=COMMAND_TIMEOUT_SECONDS,
        help=f"Per-probe timeout in seconds (default: {COMMAND_TIMEOUT_SECONDS:g})",
    )
    parser.add_argument(
        "--max-depth",
        type=lambda value: _bounded_positive_integer(value, maximum=MAX_PROFILE_DEPTH),
        default=MAX_PROFILE_DEPTH,
        help=f"Maximum parsed profile nesting depth (default: {MAX_PROFILE_DEPTH})",
    )
    parser.add_argument(
        "--max-workers",
        type=lambda value: _bounded_positive_integer(value, maximum=MAX_SCAN_WORKERS),
        default=16,
        help="Maximum parallel probe workers (default: 16)",
    )
    parser.add_argument(
        "--no-parallel",
        action="store_true",
        help="Disable parallel scanning (useful for debugging)",
    )
    parser.add_argument(
        "--include-vendored",
        action="store_true",
        help=(
            "Allow executables from vendored/project-local PATH segments such as "
            "node_modules or .venv"
        ),
    )
    parser.add_argument(
        "--redact",
        action="store_true",
        help="Replace hostname, executable paths, and raw version banners with [redacted]",
    )


def _cmd_list_profiles() -> int:
    profiles = list_builtin_profiles()
    print("Available profiles:")
    for name in profiles:
        profile = load_builtin_profile(name)
        tool_count = len(profile.tools)
        print(f"  {name:<16} {profile.description} ({tool_count} tools)")
    return 0


def _cmd_scan(args: argparse.Namespace) -> int:
    # Load profile
    try:
        if args.config:
            profile = load_custom_profile(args.config, max_depth=args.max_depth)
        elif args.profile:
            profile = load_builtin_profile(args.profile, max_depth=args.max_depth)
        else:
            profile = load_builtin_profile("full", max_depth=args.max_depth)
    except FileNotFoundError:
        if args.profile:
            print(f"Error: unknown profile '{args.profile}'", file=sys.stderr)
            print(f"Available: {', '.join(list_builtin_profiles())}", file=sys.stderr)
        else:
            print(f"Error: profile not found: {args.config}", file=sys.stderr)
        return 2
    except ValueError as exc:
        print(f"Error: invalid profile: {exc}", file=sys.stderr)
        return 2

    # Scan
    result = scan_tools(
        tools=profile.tools,
        services=profile.services,
        parallel=not args.no_parallel,
        max_workers=args.max_workers,
        include_vendored=args.include_vendored,
        timeout=args.timeout,
    )
    if args.redact:
        result = redact_scan(result)

    # Format and print
    formatter = FORMATTERS[args.format]
    print(formatter(result))

    # Check mode: exit 1 if required tools are missing
    if args.command == "check" and profile.required_tools:
        missing = []
        for tr in result.results:
            if tr.name in profile.required_tools and not tr.found:
                missing.append(tr.name)
        if missing:
            print(f"\nMissing required tools: {', '.join(missing)}", file=sys.stderr)
            return 1

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
