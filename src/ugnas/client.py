"""Low-level UGOS Pro HTTP client."""

from __future__ import annotations

import base64
import hashlib
import json
import logging
import os
import re
import time
import uuid
from collections.abc import Callable
from contextlib import suppress
from pathlib import Path
from threading import RLock
from typing import Any, BinaryIO
from urllib.parse import parse_qs, quote, urlencode, urljoin, urlparse

import requests
from Crypto.Cipher import PKCS1_v1_5
from Crypto.PublicKey import RSA

from .config import default_session_path
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
from .secure_store import protect_secret, unprotect_secret

DEFAULT_TIMEOUT = 20
AUTH_EXPIRED_CODES = {1024}
SUCCESS_CODES = {200, 1000, 1013, 1118}
logger = logging.getLogger(__name__)


def parse_share_ref(ref: str) -> tuple[str, str | None]:
    """Parse share_id and optional uglink origin from a page URL or raw id."""
    text = (ref or "").strip()
    if not text:
        raise RuntimeError("empty share page / share_id")
    parsed = urlparse(text)
    query = parse_qs(parsed.query)
    candidates = query.get("id") or query.get("share_id")
    share_id = candidates[0] if candidates else None
    if not share_id and "id=" in text:
        share_id = text.rsplit("id=", 1)[-1].split("&", 1)[0]
    if not share_id and "://" not in text and "/" not in text:
        share_id = text
    if not share_id:
        raise RuntimeError("share page URL missing id=")
    origin = None
    host = parsed.netloc or ""
    if host and "ug.link" in host and host != "ug.link":
        origin = f"{parsed.scheme or 'https'}://{host}"
    return share_id, origin


def _norm_base(url: str) -> str:
    return url.rstrip("/") + "/"


def _expiry_from(data: dict[str, Any]) -> int | None:
    for key in ("expires_at", "expire_time", "expired_at", "token_expire", "expire"):
        raw = data.get(key)
        if raw is None or raw == "":
            continue
        try:
            value = int(raw)
        except (TypeError, ValueError):
            continue
        if value > 10_000_000_000:
            value //= 1000
        if value > 1_000_000_000:
            return value
    raw_in = data.get("expires_in")
    if raw_in not in (None, ""):
        try:
            return int(time.time()) + int(raw_in)
        except (TypeError, ValueError):
            return None
    return None


def load_rsa_public_key(raw: str) -> RSA.RsaKey:
    text = raw.strip()
    if "BEGIN" in text:
        return RSA.import_key(text.encode("utf-8"))
    try:
        decoded = base64.b64decode(text)
    except Exception as exc:
        raise ValueError("x-rsa-token is not valid base64") from exc
    if b"BEGIN" in decoded:
        return RSA.import_key(decoded)
    return RSA.import_key(decoded)


def rsa_encrypt_password(public_key: RSA.RsaKey, password: str) -> str:
    cipher = PKCS1_v1_5.new(public_key)
    return base64.b64encode(cipher.encrypt(password.encode("utf-8"))).decode("ascii")


class UgnasClient:
    def __init__(
        self,
        base_url: str,
        client_id: str | None = None,
        session_path: str | Path | None = None,
        profile: str | None = None,
    ) -> None:
        self.base_url = _norm_base(base_url)
        self.client_id = client_id or str(uuid.uuid4())
        self.profile = profile
        self.session_path = Path(session_path) if session_path else default_session_path(profile)
        self.session = requests.Session()
        self.session.verify = True
        self.session.headers.update(
            {
                "UG-Agent": "PC/WEB",
                "Accept": "application/json, text/plain, */*",
                "X-Specify-Language": "zh-CN",
                "Client-Id": self.client_id,
                "Client-Version": "1.18.0",
            }
        )
        self.token: str | None = None
        self.token_where = "url"
        self.login_data: dict[str, Any] = {}
        self.username: str | None = None
        self.reused_session = False
        self._public_base: str | None = None
        self.save_dir: str | None = None
        self._password: str | None = None
        self._auth_lock = RLock()

    def url(self, path: str) -> str:
        return urljoin(self.base_url, path.lstrip("/"))

    def _auth_kwargs(self, token_auth: bool) -> dict[str, Any]:
        headers = {}
        params = {}
        if token_auth:
            if not self.token:
                raise RuntimeError("not logged in")
            # This NAS returns auth_type=header, but filemgr APIs only
            # accept the login token as a query parameter.
            params["token"] = self.token
            if self.token_where and self.token_where != "url":
                headers["X-Ugreen-Token"] = self.token
        return {"headers": headers, "params": params}

    def request(
        self,
        method: str,
        path: str,
        *,
        token_auth: bool = True,
        extra_headers: dict[str, str] | None = None,
        _reauth: bool = True,
        **kwargs: Any,
    ) -> dict[str, Any]:
        extra_params = kwargs.pop("params", {})
        timeout = kwargs.pop("timeout", DEFAULT_TIMEOUT)
        replay_positions = self._body_positions(kwargs)
        auth = self._auth_kwargs(token_auth)
        headers = {**auth["headers"], **(extra_headers or {})}
        try:
            resp = self.session.request(
                method,
                self.url(path),
                headers=headers,
                params={**auth["params"], **extra_params},
                timeout=timeout,
                **kwargs,
            )
        except requests.RequestException as exc:
            raise NetworkError(f"{method.upper()} {path} failed: {exc}") from exc
        try:
            payload = resp.json()
        except ValueError as exc:
            if token_auth and resp.status_code == 401 and _reauth and self._recover_auth():
                self._rewind_body(replay_positions)
                return self.request(
                    method,
                    path,
                    token_auth=token_auth,
                    extra_headers=extra_headers,
                    _reauth=False,
                    params=extra_params,
                    timeout=timeout,
                    **kwargs,
                )
            if resp.status_code >= 400:
                raise ApiError(
                    f"HTTP {resp.status_code} for {method.upper()} {path}",
                    status_code=resp.status_code,
                ) from exc
            raise ApiError(
                f"invalid JSON response for {method.upper()} {path}",
                status_code=resp.status_code,
            ) from exc
        code = payload.get("code")
        expired = code in AUTH_EXPIRED_CODES or (token_auth and resp.status_code == 401)
        if expired:
            if _reauth and token_auth and self._recover_auth():
                self._rewind_body(replay_positions)
                return self.request(
                    method,
                    path,
                    token_auth=token_auth,
                    extra_headers=extra_headers,
                    _reauth=False,
                    params=extra_params,
                    timeout=timeout,
                    **kwargs,
                )
            raise AuthExpired(
                f"api error {code or resp.status_code}: {payload.get('msg')}",
                code=code,
                status_code=resp.status_code,
                payload=payload,
            )
        if resp.status_code >= 400:
            raise ApiError(
                f"HTTP {resp.status_code}: {payload.get('msg') or 'request failed'}",
                code=code,
                status_code=resp.status_code,
                payload=payload,
            )
        if code not in SUCCESS_CODES:
            raise ApiError(
                f"api error {code}: {payload.get('msg')}",
                code=code,
                status_code=resp.status_code,
                payload=payload,
            )
        payload["_headers"] = {k.lower(): v for k, v in resp.headers.items()}
        return payload

    @staticmethod
    def _body_positions(kwargs: dict[str, Any]) -> list[tuple[BinaryIO, int | None]]:
        streams: list[Any] = []
        data = kwargs.get("data")
        if hasattr(data, "read"):
            streams.append(data)
        files = kwargs.get("files") or {}
        values = files.values() if isinstance(files, dict) else files
        for value in values:
            candidate = value
            if isinstance(value, tuple):
                candidate = next((item for item in value[1:] if hasattr(item, "read")), None)
            if hasattr(candidate, "read"):
                streams.append(candidate)
        result: list[tuple[BinaryIO, int | None]] = []
        for stream in streams:
            try:
                result.append((stream, int(stream.tell())))
            except (AttributeError, OSError, TypeError, ValueError):
                result.append((stream, None))
        return result

    @staticmethod
    def _rewind_body(positions: list[tuple[BinaryIO, int | None]]) -> None:
        for stream, position in positions:
            if position is None:
                raise RequestNotReplayable("authentication expired while sending a non-seekable body")
            try:
                stream.seek(position)
            except (AttributeError, OSError, TypeError, ValueError) as exc:
                raise RequestNotReplayable("could not rewind request body after authentication renewal") from exc

    def _recover_auth(self) -> bool:
        """Refresh token, then password login if the session is dead."""
        with self._auth_lock:
            if self.token:
                try:
                    payload = self.request("PUT", "/ugreen/v1/verify/refresh", _reauth=False)
                    data = payload.get("data") or {}
                    if data.get("token"):
                        self.token = data["token"]
                        self.login_data = {**self.login_data, **data}
                        self.reused_session = True
                        self.save_session()
                        return True
                except (UgnasError, ValueError, OSError):
                    logger.debug("token refresh failed; trying password login", exc_info=True)
            from .config import load_dotenv, load_saved_password

            load_dotenv()
            password = self._password or os.environ.get("UGNAS_PASSWORD") or load_saved_password(self.profile)
            username = self.username or os.environ.get("UGNAS_USERNAME")
            if not username or not password:
                return False
            try:
                self.login(username, password, reuse=False)
                return bool(self.token)
            except (UgnasError, ValueError, OSError):
                logger.debug("password re-login failed", exc_info=True)
                return False

    def load_session(self) -> dict[str, Any] | None:
        path = self.session_path
        if not path.is_file():
            return None
        try:
            data = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            return None
        if not isinstance(data, dict) or not data.get("token"):
            return None
        raw_token = data.get("token")
        if isinstance(raw_token, str) and raw_token.startswith("dpapi:"):
            try:
                data["token"] = unprotect_secret(raw_token)
            except (OSError, ValueError):
                return None
        return data

    def save_session(self) -> Path:
        if not self.token:
            raise RuntimeError("not logged in")
        data = self.login_data or {}
        stored_token = protect_secret(self.token) if os.name == "nt" else self.token
        payload = {
            "username": self.username or data.get("username"),
            "base_url": self.base_url.rstrip("/"),
            "token": stored_token,
            "auth_type": self.token_where,
            "client_id": self.client_id,
            "uglink": data.get("uglink"),
            "http_port": data.get("http_port"),
            "https_port": data.get("https_port"),
            "expires_at": _expiry_from(data),
            "saved_at": int(time.time()),
        }
        path = self.session_path
        path.parent.mkdir(parents=True, exist_ok=True)
        tmp = path.with_name(path.name + ".tmp")
        tmp.write_text(json.dumps(payload, ensure_ascii=False, indent=2), encoding="utf-8")
        tmp.replace(path)
        with suppress(OSError):
            os.chmod(path, 0o600)
        return path

    def apply_session(self, saved: dict[str, Any]) -> None:
        token = saved.get("token")
        if not token:
            raise RuntimeError("saved session has no token")
        self.token = token
        self.token_where = saved.get("auth_type") or "url"
        self.username = saved.get("username") or self.username
        if saved.get("client_id"):
            self.client_id = saved["client_id"]
            self.session.headers["Client-Id"] = self.client_id
        self.login_data = {
            "token": token,
            "auth_type": self.token_where,
            "username": self.username,
            "uglink": saved.get("uglink"),
            "http_port": saved.get("http_port"),
            "https_port": saved.get("https_port"),
            "expires_at": saved.get("expires_at"),
        }

    def _session_matches(self, saved: dict[str, Any], username: str) -> bool:
        if saved.get("username") and saved["username"] != username:
            return False
        saved_base = saved.get("base_url")
        if saved_base and _norm_base(saved_base) != self.base_url:
            return False
        expires_at = saved.get("expires_at")
        if expires_at:
            try:
                if int(expires_at) <= int(time.time()):
                    return False
            except (TypeError, ValueError):
                pass
        return True

    def _validate_session(self) -> bool:
        try:
            self.request("GET", "/ugreen/v1/filemgr/getVolumes", _reauth=False)
            return True
        except AuthExpired:
            return False
        except (UgnasError, ValueError, OSError):
            try:
                self.request("GET", "/ugreen/v1/filemgr/shareList", _reauth=False)
                return True
            except (UgnasError, ValueError, OSError):
                return False

    def try_reuse_session(self, username: str) -> bool:
        saved = self.load_session()
        if not saved or not self._session_matches(saved, username):
            return False
        self.apply_session(saved)
        if not self._validate_session():
            self.token = None
            self.login_data = {}
            self.reused_session = False
            return False
        self.reused_session = True
        return True

    def fetch_password_key(self, username: str) -> RSA.RsaKey:
        payload = self.request(
            "POST",
            "/ugreen/v1/verify/check",
            token_auth=False,
            json={"username": username},
        )
        rsa_token = payload["_headers"].get("x-rsa-token") or payload.get("xRsaToken")
        if not rsa_token:
            raise RuntimeError("verify/check did not return x-rsa-token")
        return load_rsa_public_key(rsa_token)

    def login(
        self,
        username: str,
        password: str | None = None,
        keepalive: bool = True,
        reuse: bool = True,
    ) -> dict[str, Any]:
        self.username = username
        if password:
            self._password = password
        if reuse and self.try_reuse_session(username):
            return self.login_data
        if not password:
            raise RuntimeError("no reusable session; password required")
        public_key = self.fetch_password_key(username)
        cipher_password = rsa_encrypt_password(public_key, password)
        payload = self.request(
            "POST",
            "/ugreen/v1/verify/login",
            token_auth=False,
            extra_headers={"UG-Client-Id": self.client_id},
            json={
                "username": username,
                "password": cipher_password,
                "keepalive": keepalive,
                "otp": False,
                "is_simple": False,
            },
        )
        data = payload.get("data") or {}
        self.login_data = data
        self.token = data.get("token")
        self.token_where = data.get("auth_type") or "url"
        self.reused_session = False
        if not self.token:
            raise RuntimeError("login succeeded without token field")
        self.save_session()
        return data

    def is_login(self) -> dict[str, Any]:
        return self.request("GET", "/ugreen/v1/verify/is_login")

    def refresh(self) -> dict[str, Any]:
        payload = self.request("PUT", "/ugreen/v1/verify/refresh", _reauth=False)
        data = payload.get("data") or {}
        if data.get("token"):
            self.token = data["token"]
            self.login_data = {**self.login_data, **data}
            self.save_session()
        return data

    def heartbeat(self) -> dict[str, Any]:
        return self.request("GET", "/ugreen/v1/verify/heartbeat")

    def get_root_dirs(self) -> dict[str, Any]:
        return self.request("GET", "/ugreen/v2/filemgr/getRootDirs")

    def get_volumes(self) -> list[dict[str, Any]]:
        payload = self.request("GET", "/ugreen/v1/filemgr/getVolumes")
        data = payload.get("data") or {}
        result = data.get("result") if isinstance(data, dict) else data
        return result if isinstance(result, list) else []

    def get_path_free_size(self, path: str) -> int:
        payload = self.request("POST", "/ugreen/v1/filemgr/getPathFreeSize", json={"path": path})
        data = payload.get("data") or {}
        if isinstance(data, dict) and data.get("result") is not None:
            return int(data["result"])
        if not isinstance(data, dict) and data not in (None, ""):
            return int(data)
        raise RuntimeError("getPathFreeSize missing data.result")

    def get_user_quota(self) -> dict[str, Any]:
        payload = self.request("GET", "/ugreen/v2/user/directory/quota")
        data = payload.get("data") or {}
        return data if isinstance(data, dict) else {}

    def get_size_and_count(
        self,
        path: str,
        *,
        intranet_share_id: int = 0,
        timeout: int = 60,
    ) -> dict[str, Any]:
        """Poll GET /ugreen/v2/filemgr/getSizeAndCount until status==1 (folder size job)."""
        deadline = time.time() + timeout
        while time.time() < deadline:
            payload = self.request(
                "GET",
                "/ugreen/v2/filemgr/getSizeAndCount",
                params={"path": path, "intranet_share_id": intranet_share_id},
            )
            data = payload.get("data") or {}
            if isinstance(data, dict) and int(data.get("status") or 0) == 1:
                return data
            time.sleep(0.4)
        raise OperationTimeout(f"getSizeAndCount did not finish within {timeout}s")

    def list_dir(self, path: str, page: int = 1, limit: int = 100, only_dir: bool = False) -> dict[str, Any]:
        return self.request(
            "POST",
            "/ugreen/v1/filemgr/getDirFileList",
            json={
                "path": path,
                "page": page,
                "limit": limit,
                "sort_type": "name",
                "reverse": False,
                "only_dir": only_dir,
            },
        )

    def list_dir_v2(self, path: str, page: int = 1, limit: int = 100) -> dict[str, Any]:
        return self.request(
            "POST",
            "/ugreen/v2/filemgr/getDirFileListV2",
            json={
                "path": path,
                "page": page,
                "limit": limit,
                "data_type": 0,
                "show_dir": True,
                "left_no_page_show": False,
                "left_count": 5500,
                "is_shield_recycle": False,
            },
        )

    @staticmethod
    def _result_items(payload: dict[str, Any]) -> list[dict[str, Any]]:
        data: Any = payload.get("data") or payload
        for _ in range(3):
            if isinstance(data, list):
                return [item for item in data if isinstance(item, dict)]
            if not isinstance(data, dict):
                return []
            for key in ("result", "list", "records", "items", "rows"):
                value = data.get(key)
                if isinstance(value, list):
                    return [item for item in value if isinstance(item, dict)]
            data = data.get("data")
        return []

    def iter_dir(self, path: str, *, page_size: int = 100):
        if page_size <= 0:
            raise ValueError("page_size must be greater than zero")
        page = 1
        while True:
            items = self._result_items(self.list_dir_v2(path, page=page, limit=page_size))
            yield from items
            if len(items) < page_size:
                return
            page += 1

    def get_download_token(self, paths: list[str], intranet_share_id: int = 0, coding: bool = True) -> str:
        query = "&".join(f"paths={quote(path, safe='')}" for path in paths)
        payload = self.request(
            "GET",
            f"/ugreen/v2/filemgr/getDownloadToken?{query}",
            params={"intranet_share_id": intranet_share_id, "coding": coding},
        )
        token = (payload.get("data") or {}).get("dl_token")
        if not token:
            raise RuntimeError("getDownloadToken missing data.dl_token")
        return token

    def download_url(self, paths: list[str], intranet_share_id: int = 0) -> str:
        try:
            dl_token = self.get_download_token(paths, intranet_share_id=intranet_share_id)
            return self.url(f"/ugreen/v2/filemgr/downloadFile?token={dl_token}")
        except ApiError as exc:
            if exc.status_code not in {404, 405}:
                raise
            query = "&".join(f"paths={quote(path, safe='')}" for path in paths)
            fallback = f"/ugreen/v1/filemgr/downloadFile?intranet_share_id={intranet_share_id}&{query}"
            if self.token and self.token_where == "url":
                fallback += f"&token={self.token}"
            return self.url(fallback)

    def public_base_url(self) -> str:
        if self._public_base:
            return self._public_base.rstrip("/")
        explicit = os.environ.get("UGNAS_PUBLIC_BASE_URL", "").rstrip("/")
        if explicit:
            return explicit
        uglink = (self.login_data or {}).get("uglink")
        if uglink:
            return f"https://{str(uglink).rstrip('/')}"
        parsed = urlparse(self.base_url)
        if "ug.link" in (parsed.netloc or ""):
            return f"{parsed.scheme or 'https'}://{parsed.netloc}"
        raise RuntimeError("login first, or set UGNAS_PUBLIC_BASE_URL to the uglink origin")

    def _share_page_headers(self, share_id: str, *, json_body: bool = False) -> dict[str, str]:
        origin = self.public_base_url()
        headers = {
            "Origin": origin,
            "Referer": f"{origin}/filemgr/share-download/?id={share_id}",
        }
        if json_body:
            headers["Content-Type"] = "application/json"
        return headers

    @staticmethod
    def _compact_json(body: dict[str, Any]) -> bytes:
        return json.dumps(body, ensure_ascii=False, separators=(",", ":")).encode("utf-8")

    def _public_request(
        self,
        method: str,
        path: str,
        *,
        share_id: str,
        params: dict[str, Any] | None = None,
        body: dict[str, Any] | None = None,
        timeout: int = DEFAULT_TIMEOUT,
    ) -> dict[str, Any]:
        headers = self._share_page_headers(share_id, json_body=body is not None)
        try:
            resp = self.session.request(
                method,
                urljoin(self.public_base_url().rstrip("/") + "/", path.lstrip("/")),
                headers=headers,
                params=params,
                data=self._compact_json(body) if body is not None else None,
                timeout=timeout,
            )
        except requests.RequestException as exc:
            raise NetworkError(f"public request {method.upper()} {path} failed: {exc}") from exc
        try:
            payload = resp.json()
        except ValueError as exc:
            raise ApiError("public endpoint returned invalid JSON", status_code=resp.status_code) from exc
        if resp.status_code >= 400 or payload.get("code") not in SUCCESS_CODES:
            raise ApiError(
                f"api error {payload.get('code')}: {payload.get('msg')}",
                code=payload.get("code"),
                status_code=resp.status_code,
                payload=payload,
            )
        return payload

    def query_share_info(self, share_id: str) -> dict[str, Any]:
        payload = self._public_request(
            "GET",
            "/ugreen/v1/filemgr/queryShareInfo",
            share_id=share_id,
            params={"share_id": share_id},
        )
        return payload.get("data") or {}

    def verify_share_password(self, share_id: str, password: str = "") -> dict[str, Any]:
        payload = self._public_request(
            "POST",
            "/ugreen/v1/filemgr/externalVerifySharePassword",
            share_id=share_id,
            body={"share_id": share_id, "password": password, "no_count": False},
        )
        return payload.get("data") or {}

    def add_paths_by_share_id(self, share_id: str, paths: list[str]) -> str:
        payload = self._public_request(
            "POST",
            "/ugreen/v1/filemgr/addPathsByShareId",
            share_id=share_id,
            body={"paths": paths, "share_id": share_id},
        )
        task_id = (payload.get("data") or {}).get("result")
        if not task_id:
            raise RuntimeError("addPathsByShareId missing data.result")
        return str(task_id)

    def share_download_file_url(
        self,
        share_id: str,
        task_id: str,
        *,
        password: str = "",
        coding: bool = True,
        api_base: str | None = None,
    ) -> str:
        """Build GET /ugreen/v1/filemgr/shareDownloadFile?... (same query as share page scripts)."""
        query = urlencode(
            {
                "coding": "true" if coding else "false",
                "share_id": share_id,
                "password": password or "",
                "task_id": task_id,
            }
        )
        if api_base:
            root = api_base.rstrip("/")
        else:
            try:
                root = self.public_base_url()
            except RuntimeError:
                root = self.base_url.rstrip("/")
        if root.endswith("/ugreen/v1"):
            return f"{root}/filemgr/shareDownloadFile?{query}"
        return f"{root}/ugreen/v1/filemgr/shareDownloadFile?{query}"

    def build_share_download_url(self, share_id: str, task_id: str, password: str = "") -> str:
        return self.share_download_file_url(share_id, task_id, password=password)

    def get_share_download_url(
        self,
        share_id: str,
        password: str = "",
        paths: list[str] | None = None,
    ) -> dict[str, Any]:
        share_id, origin = parse_share_ref(share_id)
        if origin and not self._public_base:
            self._public_base = origin
        info = self.query_share_info(share_id)
        verified = self.verify_share_password(share_id, password)
        files = verified.get("file_info") or []
        if not paths:
            paths = [item["path"] for item in files if item.get("path")]
        if not paths and info.get("path"):
            paths = [info["path"]]
        if not paths:
            raise RuntimeError("verifySharePassword returned no file_info.path")
        task_id = self.add_paths_by_share_id(share_id, paths)
        first = files[0] if files else {}
        return {
            "url": self.build_share_download_url(share_id, task_id, password),
            "task_id": task_id,
            "share_id": share_id,
            "paths": paths,
            "name": first.get("name") or info.get("name"),
            "size": first.get("size"),
            "files": files,
            "share_info": info,
        }

    def _stream_headers(self, url: str) -> dict[str, str]:
        headers: dict[str, str] = {}
        if (
            self.token
            and self.token_where != "url"
            and "downloadFile?token=" not in url
            and "shareDownloadFile" not in url
        ):
            headers["X-Ugreen-Token"] = self.token
        return headers

    @staticmethod
    def _local_dest_path(dest: str | Path, source_name: str | None = None) -> Path:
        dest_path = Path(dest)
        dest_str = str(dest)
        treat_as_dir = dest_str.endswith(("/", "\\")) or dest_path.is_dir()
        if not treat_as_dir and source_name and not dest_path.exists() and dest_path.suffix == "":
            treat_as_dir = True
        if treat_as_dir:
            name = Path(source_name).name if source_name else None
            if not name:
                raise RuntimeError("dest is a folder; pass a NAS file path so the local name can be inferred")
            return dest_path / name
        return dest_path

    def download_url_to_file(
        self,
        url: str,
        dest: str | Path,
        *,
        timeout: int = 300,
        chunk_size: int = 256 * 1024,
        dry_run: bool = False,
        source_name: str | None = None,
        resume: bool = True,
        progress: Callable[[int, int | None], None] | None = None,
        cancelled: Callable[[], bool] | None = None,
    ) -> dict[str, Any]:
        """Stream a share/direct URL to a local filesystem path (Windows dest is fine).

        dest may be a file path or a folder (existing dir, or trailing / \\).
        Missing folders are created. dry_run=True only HEADs (or Range 0-0).
        """
        dest_path = self._local_dest_path(dest, source_name)
        headers = self._stream_headers(url)
        if dry_run:
            try:
                resp = self.session.head(url, headers=headers, timeout=min(timeout, 20), allow_redirects=True)
                if resp.status_code == 405:
                    get_headers = {**headers, "Range": "bytes=0-0"}
                    resp = self.session.get(url, headers=get_headers, timeout=min(timeout, 20), stream=True)
                if resp.status_code >= 400:
                    raise ApiError(f"download probe returned HTTP {resp.status_code}", status_code=resp.status_code)
            except requests.RequestException as exc:
                raise NetworkError(f"download probe failed: {exc}") from exc
            finally:
                if "resp" in locals():
                    resp.close()
            return {
                "dest": str(dest_path),
                "url": resp.url,
                "status": resp.status_code,
                "content_type": resp.headers.get("Content-Type"),
                "content_length": resp.headers.get("Content-Length"),
                "content_disposition": resp.headers.get("Content-Disposition"),
                "dry_run": True,
            }
        if chunk_size <= 0:
            raise ValueError("chunk_size must be greater than zero")
        dest_path.parent.mkdir(parents=True, exist_ok=True)
        part_path = dest_path.with_name(dest_path.name + ".part")
        offset = part_path.stat().st_size if resume and part_path.is_file() else 0
        request_headers = dict(headers)
        if offset:
            request_headers["Range"] = f"bytes={offset}-"
        try:
            response = self.session.get(url, headers=request_headers, stream=True, timeout=timeout)
        except requests.RequestException as exc:
            raise NetworkError(f"download failed: {exc}") from exc
        with response as resp:
            if resp.status_code >= 400:
                raise ApiError(f"download returned HTTP {resp.status_code}", status_code=resp.status_code)
            append = bool(offset and resp.status_code == 206)
            written = offset if append else 0
            content_range = resp.headers.get("Content-Range", "")
            total: int | None = None
            match = re.search(r"/(\d+)$", content_range)
            if match:
                total = int(match.group(1))
            elif resp.headers.get("Content-Length"):
                total = written + int(resp.headers["Content-Length"])
            mode = "ab" if append else "wb"
            try:
                with part_path.open(mode) as handle:
                    for chunk in resp.iter_content(chunk_size):
                        if cancelled and cancelled():
                            raise TransferCancelled(f"download cancelled: {dest_path}")
                        if chunk:
                            handle.write(chunk)
                            written += len(chunk)
                            if progress:
                                progress(written, total)
                    handle.flush()
                    os.fsync(handle.fileno())
            except requests.RequestException as exc:
                raise NetworkError(f"download stream failed: {exc}") from exc
        actual = part_path.stat().st_size
        if total is not None and actual != total:
            raise TransferError(f"incomplete download: expected {total} bytes, got {actual}")
        part_path.replace(dest_path)
        return {
            "dest": str(dest_path),
            "bytes": actual,
            "resumed_from": offset if append else 0,
            "dry_run": False,
        }

    def download_file(self, nas_path: str, dest: str, intranet_share_id: int = 0) -> str:
        url = self.download_url([nas_path], intranet_share_id=intranet_share_id)
        try:
            return self.download_url_to_file(url, dest, source_name=nas_path)["dest"]
        except ApiError as exc:
            if exc.status_code != 401:
                raise
            fresh_url = self.download_url([nas_path], intranet_share_id=intranet_share_id)
            return self.download_url_to_file(fresh_url, dest, source_name=nas_path)["dest"]

    def add_share(
        self,
        paths: list[str],
        *,
        share_type: int = 2,
        allow_download: bool = True,
        expiration_days: int = 0,
        password: str | None = None,
        visit_limit: int | None = None,
        internal_user_names: list[str] | None = None,
        internal_group_names: list[str] | None = None,
    ) -> dict[str, Any]:
        body: dict[str, Any] = {
            "share_type": share_type,
            "paths": paths,
            "allow_download": allow_download,
            "internal_user_names": internal_user_names or [],
            "internal_group_names": internal_group_names or [],
        }
        if share_type == 2:
            body["expiration_time"] = expiration_days
            if password:
                body["password"] = password
            if visit_limit is not None:
                body["visit_limit"] = visit_limit
        payload = self.request("POST", "/ugreen/v1/filemgr/addShare", json=body)
        return payload.get("data") or {}

    def get_share(self, share_id: int | str) -> dict[str, Any]:
        payload = self.request("GET", "/ugreen/v1/filemgr/getShareMsg", params={"id": share_id})
        return payload.get("data") or {}

    def list_shares(self, keyword: str | None = None, *, page: int = 1, limit: int = 100) -> dict[str, Any]:
        params: dict[str, Any] = {"page": page, "limit": limit}
        if keyword:
            params["key_word"] = keyword
        return self.request("GET", "/ugreen/v1/filemgr/shareList", params=params)

    def share_url(self, share_id: int | str | None = None, paths: list[str] | None = None) -> str:
        if share_id is not None:
            data = self.get_share(share_id)
            link = data.get("link")
            if not link:
                raise RuntimeError("getShareMsg did not return link")
            return link
        if paths:
            data = self.add_share(paths, share_type=2, expiration_days=0)
            link = data.get("link")
            if not link:
                raise RuntimeError("addShare did not return link")
            return link
        raise RuntimeError("pass share_id or paths")

    def delete_paths(self, paths: list[str]) -> dict[str, Any]:
        return self.request("POST", "/ugreen/v1/filemgr/delPaths", json={"paths": paths})

    def rename(self, path: str, new_name: str) -> dict[str, Any]:
        payload = self.request("POST", "/ugreen/v2/filemgr/rename", json={"path": path, "new_name": new_name})
        return payload.get("data") or payload

    def copy_or_move(
        self,
        src: str | list[str],
        dest_dir: str,
        *,
        move: bool = True,
        force: bool = True,
        intranet_share_id: int = 0,
        wait_timeout: float = 60,
        poll_interval: float = 0.5,
    ) -> dict[str, Any]:
        paths = [src] if isinstance(src, str) else list(src)
        if not paths:
            raise RuntimeError("src required")
        dest_dir = dest_dir.rstrip("/")
        payload = self.request(
            "POST",
            "/ugreen/v2/filemgr/cpOrMvPath",
            json={
                "dst": dest_dir,
                "src": paths,
                "action_type": 1 if force else 0,
                "type": bool(move),
                "intranet_share_id": intranet_share_id,
            },
            timeout=180,
        )
        data = payload.get("data") or payload
        dests = [f"{dest_dir}/{item.rstrip('/').rsplit('/', 1)[-1]}" for item in paths]
        deadline = time.time() + wait_timeout
        while time.time() < deadline:
            if all(self.path_exists(item) for item in dests):
                break
            time.sleep(poll_interval)
        else:
            raise OperationTimeout(f"copy/move targets did not appear within {wait_timeout}s: {dests}")
        if isinstance(data, dict):
            data = {**data, "paths": dests}
        return data

    def resolve_share_ids(self, share_ids: str | int | list[str | int]) -> list[str]:
        raw = list(share_ids) if isinstance(share_ids, (list, tuple)) else [share_ids]
        numeric = [str(item) for item in raw if str(item).isdigit()]
        others = [str(item) for item in raw if not str(item).isdigit()]
        if not others:
            return numeric
        items: list[dict[str, Any]] = []
        page = 1
        while True:
            batch = self._result_items(self.list_shares(page=page, limit=100))
            items.extend(batch)
            if len(batch) < 100:
                break
            page += 1
        resolved = list(numeric)
        for token in others:
            match = next(
                (
                    item.get("id")
                    for item in items
                    if token in str(item.get("share_link") or item.get("link") or "")
                    or str(item.get("share_id") or "") == token
                ),
                None,
            )
            if match is None:
                raise RuntimeError(f"share not found: {token}")
            resolved.append(str(match))
        return resolved

    def delete_share(self, share_ids: str | int | list[str | int]) -> dict[str, Any]:
        ids = ",".join(self.resolve_share_ids(share_ids))
        if not ids:
            raise RuntimeError("share_id required")
        return self.request("DELETE", "/ugreen/v1/filemgr/delShare", params={"id": ids})

    def path_exists(self, path: str) -> bool:
        payload = self.request("POST", "/ugreen/v1/filemgr/pathExist", json={"path": path})
        data = payload.get("data")
        if isinstance(data, dict):
            return bool(data.get("result"))
        return bool(data)

    def ensure_dir(self, path: str) -> None:
        path = path.rstrip("/")
        if not path or self.path_exists(path):
            return
        if path.count("/") < 2:
            return
        try:
            self.request(
                "POST",
                "/ugreen/v2/filemgr/createFolder",
                json={"path": path, "no_recursive_creation": False},
            )
        except ApiError as exc:
            if exc.code == 1327 or "1327" in str(exc) or "同名" in str(exc):
                return
            raise

    def add_http_download(self, url: str, save_dir: str, force: bool = True, ensure: bool = True) -> dict[str, Any]:
        if ensure:
            self.ensure_dir(save_dir)
        payload = self.request(
            "POST",
            "/ugreen/v1/downloadCenter/download/add",
            data={
                "is_batch": "false",
                "save_dir": save_dir,
                "download_url": url,
                "force": "true" if force else "false",
            },
        )
        return payload.get("data") or payload

    def list_downloads(self) -> dict[str, Any]:
        return self.request("GET", "/ugreen/v1/downloadCenter/download/getListV3")

    def list_completed_downloads(self) -> dict[str, Any]:
        return self.request("GET", "/ugreen/v1/downloadCenter/complete/getList")

    def upload_file(
        self,
        local_path: str,
        dest_dir: str,
        filename: str | None = None,
        chunk_size: int = 8 * 1024 * 1024,
        *,
        force: bool = True,
        timeout: int = 300,
        max_retries: int = 3,
        progress: Callable[[int, int], None] | None = None,
        cancelled: Callable[[], bool] | None = None,
    ) -> dict[str, Any]:
        """Upload in resumable chunks using UGOS' two-stage protocol.

        ``fileUpload`` creates/checks the remote file with an empty multipart
        body. Chunks then go to ``fileUploadV2`` and the server-side cache is
        queried after interruptions so a retry can continue at its confirmed
        byte offset.
        """
        src = Path(local_path)
        if not src.is_file():
            raise FileNotFoundError(f"upload source is not a file: {src}")
        if chunk_size <= 0:
            raise ValueError("chunk_size must be greater than zero")
        if max_retries < 0:
            raise ValueError("max_retries must not be negative")
        dest_dir = dest_dir.replace("\\", "/").rstrip("/")
        if not dest_dir:
            raise ValueError("dest_dir required")
        name = filename or src.name
        if not name or "/" in name or "\\" in name:
            raise ValueError("filename must be a file name, not a path")
        stat = src.stat()
        total = stat.st_size
        change_time = int(stat.st_mtime)
        fingerprint = f"{self.client_id}|{src.resolve()}|{total}|{stat.st_mtime_ns}|{dest_dir}|{name}"
        name_md5 = hashlib.md5(f"{src.resolve()}{total}".encode()).hexdigest()
        file_uuid = f"{uuid.uuid5(uuid.NAMESPACE_URL, fingerprint)}_1_{name_md5}"
        action_type = 1 if force else 0

        initial_form = {
            "uuid": file_uuid,
            "dir": dest_dir,
            "action_type": str(action_type),
            "size": str(total),
            "begin_size": "0",
            "current_size": "0",
            "change_time": str(change_time),
            "filename": name,
            "resume": "true",
            "first_request": "true",
        }
        initial = self.request(
            "POST",
            "/ugreen/v1/filemgr/fileUpload",
            data=initial_form,
            files={"file": (quote(name, safe=""), b"", "application/octet-stream")},
            timeout=timeout,
        )
        initial_data = initial.get("data") or {}
        if isinstance(initial_data, dict) and initial_data.get("result"):
            return initial_data

        offset = self.get_uploaded_cache_size(dest_dir, file_uuid, total)
        if offset < 0 or offset > total:
            offset = 0
        last: dict[str, Any] = initial
        with src.open("rb") as handle:
            handle.seek(offset)
            send_empty = total == 0
            while offset < total or send_empty:
                if cancelled and cancelled():
                    raise TransferCancelled(f"upload cancelled: {src}")
                chunk = handle.read(chunk_size) if total else b""
                upload_params = {
                    "uuid": file_uuid,
                    "file_name": name,
                    "action_type": action_type,
                    "size": total,
                    "current_size": len(chunk),
                    "resume": True,
                    "dir": quote(dest_dir, safe=""),
                    "change_time": change_time,
                    "is_live_photo": False,
                    "first_request": False,
                    "begin_size": offset,
                }
                headers = {
                    "Content-Type": "application/octet-stream",
                    "Content-Disposition": f'attachment; filename="{quote(name, safe="")}"',
                    "ug-param": json.dumps(upload_params, ensure_ascii=False, separators=(",", ":")),
                }
                attempt = 0
                while True:
                    try:
                        last = self.request(
                            "POST",
                            "/ugreen/v1/filemgr/fileUploadV2",
                            data=chunk,
                            extra_headers=headers,
                            timeout=timeout,
                        )
                        offset += len(chunk)
                        break
                    except NetworkError:
                        if attempt >= max_retries:
                            raise
                        attempt += 1
                        time.sleep(min(2**attempt, 5))
                        confirmed = self.get_uploaded_cache_size(dest_dir, file_uuid, total)
                        if confirmed > offset:
                            offset = confirmed
                            handle.seek(offset)
                            break
                if progress:
                    progress(offset, total)
                send_empty = False
        data = last.get("data") or last
        if isinstance(data, dict) and not data.get("result"):
            data = {**data, "result": f"{dest_dir}/{name}"}
        return data

    def get_uploaded_cache_size(self, dest_dir: str, file_uuid: str, total: int) -> int:
        """Return the server-confirmed next byte offset for an upload task."""
        try:
            payload = self.request(
                "GET",
                "/ugreen/v1/filemgr/getUpdateTmpInfo",
                params={"dir": dest_dir, "uuid": file_uuid, "size": total},
                timeout=30,
            )
        except ApiError as exc:
            if exc.status_code in {404, 405}:
                return 0
            raise
        data = payload.get("data") or {}
        result = data.get("result") if isinstance(data, dict) else data
        try:
            return max(0, int(result or 0))
        except (TypeError, ValueError):
            return 0
