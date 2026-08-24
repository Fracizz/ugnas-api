"""Structured exceptions exposed by ugnas-api."""

from __future__ import annotations

from typing import Any


class UgnasError(RuntimeError):
    """Base class for errors raised by this package."""


class ApiError(UgnasError):
    """UGOS returned an unsuccessful API or HTTP response."""

    def __init__(
        self,
        message: str,
        *,
        code: int | str | None = None,
        status_code: int | None = None,
        payload: dict[str, Any] | None = None,
    ) -> None:
        super().__init__(message)
        self.code = code
        self.status_code = status_code
        self.payload = payload or {}


class AuthExpired(ApiError):
    """Login token was rejected and could not be renewed."""


class NetworkError(UgnasError):
    """The NAS or public share endpoint could not be reached."""


class OperationTimeout(UgnasError):
    """An asynchronous NAS operation did not finish before its deadline."""


class RequestNotReplayable(UgnasError):
    """Authentication expired while a non-seekable request body was in flight."""


class TransferError(UgnasError):
    """A file transfer failed validation or was interrupted."""


class TransferCancelled(TransferError):
    """A caller cancelled an upload or download."""
