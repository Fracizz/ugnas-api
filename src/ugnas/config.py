"""Local config, .env, and session path resolution.

Profiles live under ~/.ugnas/profiles/<name>. On Windows, newly saved
passwords use DPAPI. Existing cwd/captures and ~/.ugnas files remain readable
for backwards compatibility.
"""

from __future__ import annotations

import json
import os
import re
from contextlib import suppress
from pathlib import Path
from typing import Any

from .secure_store import load_encrypted_password, save_encrypted_password

ENV_MAP = {
    "base_url": "UGNAS_BASE_URL",
    "username": "UGNAS_USERNAME",
    "client_id": "UGNAS_CLIENT_ID",
    "public_base_url": "UGNAS_PUBLIC_BASE_URL",
    "save_dir": "UGNAS_SAVE_DIR",
    "session_file": "UGNAS_SESSION_FILE",
}


def data_dir() -> Path:
    return Path(os.environ.get("UGNAS_HOME") or (Path.home() / ".ugnas"))


def profile_name(profile: str | None = None) -> str:
    name = profile or os.environ.get("UGNAS_PROFILE") or "default"
    if not re.fullmatch(r"[A-Za-z0-9_.-]+", name):
        raise ValueError("profile may contain only letters, digits, dot, underscore and hyphen")
    return name


def profile_dir(profile: str | None = None) -> Path:
    return data_dir() / "profiles" / profile_name(profile)


def _cwd_captures() -> Path:
    return Path.cwd() / "captures"


def default_env_path(profile: str | None = None) -> Path:
    explicit = os.environ.get("UGNAS_ENV_FILE")
    if explicit:
        return Path(explicit)
    profile_path = profile_dir(profile) / ".env"
    if profile is not None or os.environ.get("UGNAS_PROFILE"):
        return profile_path
    candidates = [profile_path, Path.cwd() / ".env", data_dir() / ".env"]
    return next((path for path in candidates if path.is_file()), candidates[0])


def default_config_path(profile: str | None = None) -> Path:
    explicit = os.environ.get("UGNAS_CONFIG_FILE")
    if explicit:
        return Path(explicit)
    profile_path = profile_dir(profile) / "config.json"
    if profile is not None or os.environ.get("UGNAS_PROFILE"):
        return profile_path
    candidates = [
        profile_path,
        _cwd_captures() / ".ugnas_config.json",
        data_dir() / "config.json",
    ]
    return next((path for path in candidates if path.is_file()), candidates[0])


def default_session_path(profile: str | None = None) -> Path:
    explicit = os.environ.get("UGNAS_SESSION_FILE")
    if explicit:
        return Path(explicit)
    profile_path = profile_dir(profile) / "session.json"
    if profile is not None or os.environ.get("UGNAS_PROFILE"):
        return profile_path
    candidates = [
        profile_path,
        _cwd_captures() / ".ugnas_session.json",
        data_dir() / "session.json",
    ]
    return next((path for path in candidates if path.is_file()), candidates[0])


def credential_path(profile: str | None = None) -> Path:
    return profile_dir(profile) / "credentials.json"


def load_saved_password(profile: str | None = None) -> str | None:
    return load_encrypted_password(credential_path(profile))


def save_password(password: str, profile: str | None = None) -> Path:
    if os.name == "nt":
        return save_encrypted_password(credential_path(profile), password)
    return upsert_dotenv({"UGNAS_PASSWORD": password}, profile_dir(profile) / ".env")


def load_dotenv(
    path: str | Path | None = None,
    *,
    override: bool = False,
    profile: str | None = None,
) -> Path | None:
    file_path = Path(path) if path else default_env_path(profile)
    if not file_path.is_file():
        return None
    for raw in file_path.read_text(encoding="utf-8").splitlines():
        line = raw.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        key = key.strip()
        value = value.strip().strip("'").strip('"')
        if not key:
            continue
        if override or key not in os.environ:
            os.environ[key] = value
    return file_path


def upsert_dotenv(
    updates: dict[str, str],
    path: str | Path | None = None,
    *,
    profile: str | None = None,
) -> Path:
    file_path = Path(path) if path else default_env_path(profile)
    file_path.parent.mkdir(parents=True, exist_ok=True)
    lines = file_path.read_text(encoding="utf-8").splitlines() if file_path.is_file() else []
    seen: set[str] = set()
    out: list[str] = []
    for line in lines:
        stripped = line.strip()
        if stripped and not stripped.startswith("#") and "=" in stripped:
            key = stripped.split("=", 1)[0].strip()
            if key in updates:
                out.append(f"{key}={updates[key]}")
                seen.add(key)
                continue
        out.append(line)
    for key, value in updates.items():
        if key not in seen:
            out.append(f"{key}={value}")
    file_path.write_text("\n".join(out).rstrip() + "\n", encoding="utf-8")
    with suppress(OSError):
        os.chmod(file_path, 0o600)
    return file_path


def load_config(path: str | Path | None = None, *, profile: str | None = None) -> dict[str, Any]:
    load_dotenv(profile=profile)
    file_path = Path(path) if path else default_config_path(profile)
    data: dict[str, Any] = {}
    if file_path.is_file():
        loaded = json.loads(file_path.read_text(encoding="utf-8"))
        if isinstance(loaded, dict):
            data.update(loaded)
    data.pop("password", None)
    for key, env_name in ENV_MAP.items():
        value = os.environ.get(env_name)
        if value:
            data[key] = value
    if os.environ.get("UGNAS_INSECURE") == "1":
        data["insecure"] = True
    return data


def save_config(
    data: dict[str, Any],
    path: str | Path | None = None,
    *,
    profile: str | None = None,
) -> Path:
    file_path = Path(path) if path else default_config_path(profile)
    file_path.parent.mkdir(parents=True, exist_ok=True)
    current: dict[str, Any] = {}
    if file_path.is_file():
        try:
            loaded = json.loads(file_path.read_text(encoding="utf-8"))
            if isinstance(loaded, dict):
                current = loaded
        except json.JSONDecodeError:
            current = {}
    password = data.get("password")
    current.update({key: value for key, value in data.items() if value is not None and key != "password"})
    current.pop("password", None)
    tmp = file_path.with_name(file_path.name + ".tmp")
    tmp.write_text(json.dumps(current, ensure_ascii=False, indent=2), encoding="utf-8")
    tmp.replace(file_path)
    if password:
        save_password(str(password), profile)
    with suppress(OSError):
        os.chmod(file_path, 0o600)
    return file_path


def connect(
    *,
    reuse: bool = True,
    config_path: str | Path | None = None,
    base_url: str | None = None,
    username: str | None = None,
    password: str | None = None,
    save_dir: str | None = None,
    public_base_url: str | None = None,
    insecure: bool | None = None,
    client_id: str | None = None,
    session_path: str | Path | None = None,
    profile: str | None = None,
):
    """Login. Explicit args override config / env. Password is not written to disk."""
    from .client import UgnasClient

    cfg = load_config(config_path, profile=profile)
    base_url = base_url or cfg.get("base_url") or os.environ.get("UGNAS_BASE_URL")
    username = username or cfg.get("username") or os.environ.get("UGNAS_USERNAME")
    password = password or os.environ.get("UGNAS_PASSWORD") or load_saved_password(profile)
    if not base_url or not username:
        raise RuntimeError("missing base_url/username: pass them to connect() or set UGNAS_BASE_URL / UGNAS_USERNAME")
    session_path = session_path or cfg.get("session_file") or default_session_path(profile)
    client = UgnasClient(
        base_url,
        client_id or cfg.get("client_id") or os.environ.get("UGNAS_CLIENT_ID"),
        session_path=session_path,
        profile=profile,
    )
    if insecure if insecure is not None else (cfg.get("insecure") or os.environ.get("UGNAS_INSECURE") == "1"):
        client.session.verify = False
    public = public_base_url or cfg.get("public_base_url")
    if public:
        client._public_base = str(public).rstrip("/")
    client.save_dir = save_dir or cfg.get("save_dir")
    client.login(username, password, reuse=reuse)
    return client


def connect_from_env(*, reuse: bool = True, profile: str | None = None):
    return connect(reuse=reuse, profile=profile)
