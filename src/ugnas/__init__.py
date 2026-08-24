"""UGOS Pro unofficial client: login, download center, share, direct links."""

from .api import Ugnas
from .client import UgnasClient
from .config import connect, load_config, save_config
from .errors import (
    ApiError,
    AuthExpired,
    NetworkError,
    OperationTimeout,
    RequestNotReplayable,
    TransferCancelled,
    TransferError,
    UgnasError,
)

__all__ = [
    "ApiError",
    "AuthExpired",
    "NetworkError",
    "OperationTimeout",
    "RequestNotReplayable",
    "TransferCancelled",
    "TransferError",
    "Ugnas",
    "UgnasClient",
    "UgnasError",
    "connect",
    "load_config",
    "save_config",
]
__version__ = "0.3.0"
