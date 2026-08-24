"""Compatibility shim. Prefer: from ugnas import Ugnas"""

from ugnas import AuthExpired, Ugnas, UgnasClient, connect, load_config, save_config

__all__ = ["AuthExpired", "Ugnas", "UgnasClient", "connect", "load_config", "save_config"]
