"""Compatibility shim. Prefer: from ugnas import UgnasClient, connect"""

from ugnas.client import (
    AUTH_EXPIRED_CODES,
    DEFAULT_TIMEOUT,
    AuthExpired,
    UgnasClient,
    load_rsa_public_key,
    rsa_encrypt_password,
)
from ugnas.cli import _public_preview
from ugnas.config import (
    connect,
    connect_from_env,
    load_config,
    load_dotenv,
    save_config,
    upsert_dotenv,
)

__all__ = [
    "AUTH_EXPIRED_CODES",
    "DEFAULT_TIMEOUT",
    "AuthExpired",
    "UgnasClient",
    "connect",
    "connect_from_env",
    "load_config",
    "load_dotenv",
    "load_rsa_public_key",
    "rsa_encrypt_password",
    "save_config",
    "upsert_dotenv",
    "_public_preview",
]

if __name__ == "__main__":
    from ugnas.cli import main

    raise SystemExit(main())

