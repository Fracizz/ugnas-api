"""Command-line entry: ugnas <command>."""

from __future__ import annotations

import argparse
import getpass
import json
import sys
from typing import Any
from urllib.parse import parse_qsl, urlencode, urlsplit, urlunsplit

import requests

from . import __version__
from .api import Ugnas
from .config import load_config, save_config

SENSITIVE_KEYS = ("token", "password", "secret", "sign", "auth", "sn", "mail", "phone", "task_id")


def _redact_url(value: str) -> str:
    if "://" not in value or "?" not in value:
        return value
    try:
        parts = urlsplit(value)
        query = []
        for key, item in parse_qsl(parts.query, keep_blank_values=True):
            query.append((key, "<redacted>" if any(token in key.lower() for token in SENSITIVE_KEYS) else item))
        return urlunsplit((parts.scheme, parts.netloc, parts.path, urlencode(query), parts.fragment))
    except ValueError:
        return value


def _public_preview(data: Any, *, show_sensitive: bool = False) -> Any:
    if show_sensitive:
        return data
    if isinstance(data, dict):
        redacted = {}
        for key, value in data.items():
            key_l = str(key).lower()
            if any(token in key_l for token in SENSITIVE_KEYS):
                redacted[key] = f"<redacted:{len(str(value))}>"
            else:
                redacted[key] = _public_preview(value)
        return redacted
    if isinstance(data, list):
        return [_public_preview(item) for item in data]
    if isinstance(data, str):
        return _redact_url(data)
    return data


def _print(data: Any, *, show_sensitive: bool = False) -> None:
    print(json.dumps(_public_preview(data, show_sensitive=show_sensitive), ensure_ascii=False, indent=2))


def main() -> int:
    parser = argparse.ArgumentParser(prog="ugnas", description="UGOS Pro unofficial client")
    parser.add_argument(
        "command",
        nargs="?",
        default="login",
        choices=[
            "config-save",
            "config-show",
            "login",
            "pull",
            "download",
            "upload",
            "rename",
            "move",
            "copy",
            "share",
            "share-direct",
            "download-share",
            "unshare",
            "space",
            "size",
        ],
    )
    parser.add_argument("--base-url")
    parser.add_argument("--username")
    parser.add_argument("--password")
    parser.add_argument("--password-stdin", action="store_true")
    parser.add_argument("--prompt-password", action="store_true")
    parser.add_argument("--share-password")
    parser.add_argument("--profile")
    parser.add_argument("--public-base-url")
    parser.add_argument("--insecure", action="store_true")
    parser.add_argument("--url")
    parser.add_argument("--save-dir")
    parser.add_argument("--path")
    parser.add_argument("--dest")
    parser.add_argument("--share-id")
    parser.add_argument("--name")
    parser.add_argument("--wait", action="store_true")
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--no-direct", action="store_true")
    parser.add_argument("--no-force", action="store_true")
    parser.add_argument("--progress", action="store_true")
    parser.add_argument("--show-sensitive", action="store_true")
    parser.add_argument("--version", action="version", version=f"%(prog)s {__version__}")
    args = parser.parse_args()

    password = args.password
    if args.password_stdin:
        password = sys.stdin.readline().rstrip("\r\n")
    elif args.prompt_password:
        password = getpass.getpass("UGNAS password: ")
    elif args.password:
        sys.stderr.write(
            "warning: --password is visible in process arguments; prefer --password-stdin or --prompt-password\n"
        )

    def progress(done: int, total: int | None) -> None:
        if not args.progress:
            return
        suffix = f"/{total}" if total is not None else ""
        sys.stderr.write(f"\rtransferred {done}{suffix} bytes")
        if total is not None and done >= total:
            sys.stderr.write("\n")

    if args.command == "config-save":
        path = save_config(
            {
                "base_url": args.base_url,
                "username": args.username,
                "password": password,
                "public_base_url": args.public_base_url,
                "save_dir": args.save_dir,
                "insecure": True if args.insecure else None,
            },
            profile=args.profile,
        )
        print("saved", path)
        if password:
            print("password saved with DPAPI on Windows (0600 .env fallback elsewhere)")
        return 0
    if args.command == "config-show":
        _print(load_config(profile=args.profile), show_sensitive=args.show_sensitive)
        return 0

    nas = Ugnas.connect(
        profile=args.profile,
        base_url=args.base_url,
        username=args.username,
        password=password,
        public_base_url=args.public_base_url,
        save_dir=args.save_dir,
        insecure=True if args.insecure else None,
    )
    print("login ok", "(reused session)" if nas.client.reused_session else "(password login)")
    if args.command == "login":
        _print(nas.client.login_data, show_sensitive=args.show_sensitive)
        return 0
    if args.command == "pull":
        if not args.url:
            raise SystemExit("pull needs --url")
        _print(
            nas.pull(args.url, args.save_dir, force=not args.no_force, wait=args.wait),
            show_sensitive=args.show_sensitive,
        )
        return 0
    if args.command == "download":
        if not args.path or not args.dest:
            raise SystemExit("download needs --path and --dest")
        result = nas.download(args.path, args.dest, dry_run=args.dry_run, progress=progress)
        _print(result if isinstance(result, dict) else {"dest": result}, show_sensitive=args.show_sensitive)
        return 0
    if args.command == "upload":
        if not args.path or not args.dest:
            raise SystemExit("upload needs --path (local file) and --dest (NAS folder or file path)")
        _print(
            nas.upload(args.path, args.dest, filename=args.name, force=not args.no_force, progress=progress),
            show_sensitive=args.show_sensitive,
        )
        return 0
    if args.command == "rename":
        if not args.path or not args.name:
            raise SystemExit("rename needs --path and --name")
        _print(nas.rename(args.path, args.name))
        return 0
    if args.command == "move":
        if not args.path or not args.dest:
            raise SystemExit("move needs --path and --dest")
        _print(nas.move(args.path, args.dest, force=not args.no_force), show_sensitive=args.show_sensitive)
        return 0
    if args.command == "copy":
        if not args.path or not args.dest:
            raise SystemExit("copy needs --path and --dest")
        _print(nas.copy(args.path, args.dest, force=not args.no_force), show_sensitive=args.show_sensitive)
        return 0
    if args.command == "share":
        if not args.path:
            raise SystemExit("share needs --path")
        _print(
            nas.share_file(args.path, password=args.share_password, direct=not args.no_direct),
            show_sensitive=args.show_sensitive,
        )
        return 0
    if args.command == "share-direct":
        ref = args.share_id or args.url
        if not ref:
            raise SystemExit("share-direct needs --share-id or --url (share page)")
        _print(
            nas.share_direct(ref, password=args.share_password or "", paths=[args.path] if args.path else None),
            show_sensitive=args.show_sensitive,
        )
        return 0
    if args.command == "download-share":
        ref = args.share_id or args.url
        if not ref or not args.dest:
            raise SystemExit("download-share needs --url/--share-id and --dest")
        _print(
            nas.download_share(
                ref,
                args.dest,
                password=args.share_password or "",
                dry_run=args.dry_run,
                paths=[args.path] if args.path else None,
            ),
            show_sensitive=args.show_sensitive,
        )
        return 0
    if args.command == "unshare":
        if not args.share_id:
            raise SystemExit("unshare needs --share-id")
        _print(nas.unshare(args.share_id))
        return 0
    if args.command == "space":
        if args.path:
            _print(nas.free(args.path))
        else:
            _print(nas.space())
        return 0
    if args.command == "size":
        if not args.path:
            raise SystemExit("size needs --path")
        _print(nas.size(args.path))
        return 0
    raise SystemExit(f"unknown command: {args.command}")


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except requests.exceptions.SSLError:
        sys.stderr.write("TLS verify failed. Set insecure=true in config or UGNAS_INSECURE=1\n")
        raise
