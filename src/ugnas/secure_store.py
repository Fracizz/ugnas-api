"""Small dependency-free credential store.

Windows uses the current user's DPAPI key. Other platforms keep the existing
0600 dotenv fallback so the package does not grow a platform keyring dependency.
"""

from __future__ import annotations

import base64
import ctypes
import json
import os
from contextlib import suppress
from ctypes import wintypes
from pathlib import Path

if os.name == "nt":

    class _DataBlob(ctypes.Structure):
        _fields_ = [("cbData", wintypes.DWORD), ("pbData", ctypes.POINTER(ctypes.c_ubyte))]


def _blob(data: bytes):
    buffer = ctypes.create_string_buffer(data)
    return _DataBlob(len(data), ctypes.cast(buffer, ctypes.POINTER(ctypes.c_ubyte))), buffer


def protect_secret(value: str) -> str:
    if os.name != "nt":
        raise OSError("DPAPI is only available on Windows")
    source, keepalive = _blob(value.encode("utf-8"))
    output = _DataBlob()
    crypt32 = ctypes.windll.crypt32
    crypt32.CryptProtectData.argtypes = [
        ctypes.POINTER(_DataBlob),
        wintypes.LPCWSTR,
        ctypes.POINTER(_DataBlob),
        wintypes.LPVOID,
        wintypes.LPVOID,
        wintypes.DWORD,
        ctypes.POINTER(_DataBlob),
    ]
    crypt32.CryptProtectData.restype = wintypes.BOOL
    if not crypt32.CryptProtectData(
        ctypes.byref(source),
        None,
        None,
        None,
        None,
        0,
        ctypes.byref(output),
    ):
        raise ctypes.WinError()
    try:
        encrypted = ctypes.string_at(output.pbData, output.cbData)
        return "dpapi:" + base64.b64encode(encrypted).decode("ascii")
    finally:
        kernel32 = ctypes.windll.kernel32
        kernel32.LocalFree.argtypes = [wintypes.HLOCAL]
        kernel32.LocalFree.restype = wintypes.HLOCAL
        kernel32.LocalFree(ctypes.cast(output.pbData, wintypes.HLOCAL))
        del keepalive


def unprotect_secret(value: str) -> str:
    if not value.startswith("dpapi:"):
        raise ValueError("unsupported credential encoding")
    if os.name != "nt":
        raise OSError("DPAPI credential can only be opened on Windows")
    source, keepalive = _blob(base64.b64decode(value[6:]))
    output = _DataBlob()
    crypt32 = ctypes.windll.crypt32
    crypt32.CryptUnprotectData.argtypes = [
        ctypes.POINTER(_DataBlob),
        ctypes.POINTER(wintypes.LPWSTR),
        ctypes.POINTER(_DataBlob),
        wintypes.LPVOID,
        wintypes.LPVOID,
        wintypes.DWORD,
        ctypes.POINTER(_DataBlob),
    ]
    crypt32.CryptUnprotectData.restype = wintypes.BOOL
    if not crypt32.CryptUnprotectData(
        ctypes.byref(source),
        None,
        None,
        None,
        None,
        0,
        ctypes.byref(output),
    ):
        raise ctypes.WinError()
    try:
        return ctypes.string_at(output.pbData, output.cbData).decode("utf-8")
    finally:
        kernel32 = ctypes.windll.kernel32
        kernel32.LocalFree.argtypes = [wintypes.HLOCAL]
        kernel32.LocalFree.restype = wintypes.HLOCAL
        kernel32.LocalFree(ctypes.cast(output.pbData, wintypes.HLOCAL))
        del keepalive


def save_encrypted_password(path: Path, password: str) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    payload = {"password": protect_secret(password)}
    temporary = path.with_name(path.name + ".tmp")
    temporary.write_text(json.dumps(payload, indent=2), encoding="utf-8")
    temporary.replace(path)
    with suppress(OSError):
        os.chmod(path, 0o600)
    return path


def load_encrypted_password(path: Path) -> str | None:
    if not path.is_file():
        return None
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
        raw = payload.get("password") if isinstance(payload, dict) else None
        return unprotect_secret(raw) if isinstance(raw, str) else None
    except (OSError, ValueError, json.JSONDecodeError):
        return None
