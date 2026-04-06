"""Command line interface for devcap."""

from __future__ import annotations

import argparse
import sys

from .formatters import FORMATTERS
from .profile_loader import list_builtin_profiles, load_builtin_profile, load_custom_profile
from .scanner import scan_tools


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
    parser.add_argument(
        "--profile",
        help="Built-in profile name (e.g. python-dev, devops, full)",
    )
    parser.add_argument(
        "--config",
        help="Path to a custom TOML profile file",
    )
    parser.add_argument(
        "--no-parallel",
        action="store_true",
        help="Disable parallel scanning (useful for debugging)",
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
    if args.config:
        profile = load_custom_profile(args.config)
    elif args.profile:
        try:
            profile = load_builtin_profile(args.profile)
        except FileNotFoundError:
            print(f"Error: unknown profile '{args.profile}'", file=sys.stderr)
            print(f"Available: {', '.join(list_builtin_profiles())}", file=sys.stderr)
            return 2
    else:
        profile = load_builtin_profile("full")

    # Scan
    result = scan_tools(
        tools=profile.tools,
        services=profile.services,
        parallel=not args.no_parallel,
    )

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
