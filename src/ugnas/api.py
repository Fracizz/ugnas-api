"""Public facade over UgnasClient."""

from __future__ import annotations

import time
from collections.abc import Callable, Iterator
from pathlib import Path
from typing import Any

from .client import UgnasClient
from .config import connect, load_config
from .errors import ApiError

DictItems = list[dict[str, Any]]
Paths = list[str]
ShareIds = list[str | int]


class Ugnas:
    """Logged-in capability facade."""

    def __init__(self, client: UgnasClient, config: dict[str, Any] | None = None) -> None:
        self.client = client
        self.config = config or {}

    @classmethod
    def connect(
        cls,
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
    ) -> Ugnas:
        cfg = load_config(config_path, profile=profile)
        if base_url:
            cfg["base_url"] = base_url
        if username:
            cfg["username"] = username
        if save_dir:
            cfg["save_dir"] = save_dir
        if public_base_url:
            cfg["public_base_url"] = public_base_url
        if insecure is not None:
            cfg["insecure"] = insecure
        return cls(
            connect(
                reuse=reuse,
                config_path=config_path,
                base_url=base_url,
                username=username,
                password=password,
                save_dir=save_dir,
                public_base_url=public_base_url,
                insecure=insecure,
                client_id=client_id,
                session_path=session_path,
                profile=profile,
            ),
            cfg,
        )

    @property
    def default_save_dir(self) -> str:
        return self.client.save_dir or self.config.get("save_dir") or ""

    def list(self, path: str, page: int = 1, limit: int = 100) -> dict[str, Any]:
        return self.client.list_dir_v2(path, page=page, limit=limit)

    def iter(self, path: str, *, page_size: int = 100) -> Iterator[dict[str, Any]]:
        """Iterate every item in a directory across API pages."""
        return self.client.iter_dir(path, page_size=page_size)

    def list_all(self, path: str, *, page_size: int = 100) -> DictItems:
        return list(self.iter(path, page_size=page_size))

    def exists(self, path: str) -> bool:
        return self.client.path_exists(path)

    def space(self) -> dict[str, Any]:
        volumes = []
        for item in self.client.get_volumes():
            free = item.get("free")
            total = item.get("all") if item.get("all") is not None else item.get("total")
            used = item.get("used")
            volumes.append(
                {
                    "name": item.get("name"),
                    "path": item.get("path"),
                    "fs_type": item.get("fs_type"),
                    "free": free,
                    "used": used,
                    "total": total,
                }
            )
        return {"volumes": volumes, "quota": self.client.get_user_quota()}

    def free(self, path: str | None = None) -> dict[str, Any]:
        target = path or self.default_save_dir
        if not target:
            raise RuntimeError("path required (or set save_dir)")
        try:
            free = self.client.get_path_free_size(target)
            return {"path": target, "free": free}
        except ApiError:
            space = self.space()
            match = None
            for vol in space["volumes"]:
                vol_path = str(vol.get("path") or "")
                if (
                    vol_path
                    and (target == vol_path or target.startswith(vol_path.rstrip("/") + "/"))
                    and (match is None or len(vol_path) > len(str(match.get("path") or "")))
                ):
                    match = vol
            if match and match.get("free") is not None:
                return {"path": target, "free": match["free"], "volume": match["path"]}
            raise

    def size(self, path: str, *, timeout: int = 60) -> dict[str, Any]:
        """Occupied size of a file or folder (official properties / startQuerySize)."""
        data = self.client.get_size_and_count(path, timeout=timeout)
        return {
            "path": path,
            "size": data.get("size"),
            "takes_space": data.get("takes_space"),
            "file_count": data.get("file_count"),
            "dir_count": data.get("dir_count"),
            "hide_count": data.get("hide_count"),
            "status": data.get("status"),
        }

    def mkdir(self, path: str) -> None:
        self.client.ensure_dir(path)

    def delete(self, paths: Paths) -> dict[str, Any]:
        if not paths:
            raise ValueError("paths required")
        return self.client.delete_paths(paths)

    def upload(
        self,
        local_path: str,
        dest: str,
        filename: str | None = None,
        *,
        force: bool = True,
        dest_is_dir: bool | None = None,
        chunk_size: int = 8 * 1024 * 1024,
        progress: Callable[[int, int], None] | None = None,
        cancelled: Callable[[], bool] | None = None,
    ) -> dict[str, Any]:
        """Upload a file with explicit directory/full-path control."""
        local = Path(local_path)
        dest_path = dest.replace("\\", "/").rstrip("/")
        explicit_file = dest_is_dir is False or (dest_is_dir is None and dest_path.endswith("/" + local.name))
        if explicit_file:
            if "/" not in dest_path:
                raise ValueError("full NAS destination must include a parent directory")
            dest_dir, dest_name = dest_path.rsplit("/", 1)
            filename = filename or dest_name
        else:
            dest_dir = dest_path
            filename = filename or local.name
        self.mkdir(dest_dir)
        data = self.client.upload_file(
            str(local),
            dest_dir,
            filename=filename,
            force=force,
            chunk_size=chunk_size,
            progress=progress,
            cancelled=cancelled,
        )
        nas_path = data.get("result") if isinstance(data, dict) else None
        if not isinstance(nas_path, str) or not nas_path.startswith("/"):
            nas_path = f"{dest_dir.rstrip('/')}/{filename}"
        return {"path": nas_path, "result": data}

    def upload_to_dir(
        self,
        local_path: str,
        dest_dir: str,
        *,
        filename: str | None = None,
        force: bool = True,
        chunk_size: int = 8 * 1024 * 1024,
        progress: Callable[[int, int], None] | None = None,
        cancelled: Callable[[], bool] | None = None,
    ) -> dict[str, Any]:
        return self.upload(
            local_path,
            dest_dir,
            filename,
            force=force,
            dest_is_dir=True,
            chunk_size=chunk_size,
            progress=progress,
            cancelled=cancelled,
        )

    def upload_to_path(
        self,
        local_path: str,
        dest_path: str,
        *,
        force: bool = True,
        chunk_size: int = 8 * 1024 * 1024,
        progress: Callable[[int, int], None] | None = None,
        cancelled: Callable[[], bool] | None = None,
    ) -> dict[str, Any]:
        return self.upload(
            local_path,
            dest_path,
            force=force,
            dest_is_dir=False,
            chunk_size=chunk_size,
            progress=progress,
            cancelled=cancelled,
        )

    def rename(self, path: str, new_name: str) -> dict[str, Any]:
        return self.client.rename(path, new_name)

    def move(self, src: str | Paths, dest_dir: str, *, force: bool = True) -> dict[str, Any]:
        self.mkdir(dest_dir)
        return self.client.copy_or_move(src, dest_dir, move=True, force=force)

    def copy(self, src: str | Paths, dest_dir: str, *, force: bool = True) -> dict[str, Any]:
        self.mkdir(dest_dir)
        return self.client.copy_or_move(src, dest_dir, move=False, force=force)

    def unshare(self, share_ids: str | int | ShareIds) -> dict[str, Any]:
        return self.client.delete_share(share_ids)

    def pull(
        self,
        url: str,
        save_dir: str | None = None,
        *,
        force: bool = True,
        wait: bool = False,
        timeout: int = 600,
    ) -> dict[str, Any]:
        dest = save_dir or self.default_save_dir
        if not dest:
            raise RuntimeError("save_dir required (or set save_dir in config / UGNAS_SAVE_DIR)")
        added = self.client.add_http_download(url, dest, force=force)
        result = {"save_dir": dest, "task": added}
        if wait:
            result["nas_path"] = self._wait_download(added, dest, timeout=timeout)
        return result

    def download(
        self,
        nas_path: str,
        dest: str,
        *,
        dry_run: bool = False,
        progress: Callable[[int, int | None], None] | None = None,
    ) -> dict[str, Any] | str:
        """NAS path → local file. Always mints a fresh dl_token; do not cache download URLs."""
        if dry_run:
            url = self.client.download_url([nas_path])
            return self.client.download_url_to_file(url, dest, dry_run=True, source_name=nas_path)
        url = self.client.download_url([nas_path])
        try:
            result = self.client.download_url_to_file(url, dest, source_name=nas_path, progress=progress)
        except ApiError as exc:
            if exc.status_code != 401:
                raise
            fresh_url = self.client.download_url([nas_path])
            result = self.client.download_url_to_file(fresh_url, dest, source_name=nas_path, progress=progress)
        return str(result["dest"])

    def share(
        self,
        paths: Paths,
        *,
        expiration_days: int = 0,
        password: str | None = None,
        allow_download: bool = True,
    ) -> dict[str, Any]:
        data = self.client.add_share(
            paths,
            share_type=2,
            expiration_days=expiration_days,
            password=password,
            allow_download=allow_download,
        )
        link = data.get("link") or data.get("share_link") or ""
        public_id = data.get("share_id")
        if public_id is None and "id=" in link:
            public_id = link.rsplit("id=", 1)[-1].split("&", 1)[0]
        numeric_id = data.get("id")
        share_id = numeric_id if numeric_id not in (None, "") else public_id
        if public_id is not None:
            data["share_id"] = public_id
        if share_id is not None:
            data["id"] = share_id
        if share_id is not None:
            try:
                again = self.client.get_share(share_id)
                link = again.get("link") or link
                data = {**data, **again, "link": link, "id": again.get("id") or share_id}
            except ApiError:
                data["id"] = share_id
                data["link"] = link
        return data

    def share_direct(self, share: str, password: str = "", paths: Paths | None = None) -> dict[str, Any]:
        """Mint a one-shot direct URL from share_id or a share page. Do not cache the result."""
        return self.client.get_share_download_url(share, password=password, paths=paths)

    def download_share(
        self,
        share: str,
        dest: str,
        *,
        password: str = "",
        paths: Paths | None = None,
        dry_run: bool = False,
    ) -> dict[str, Any]:
        """Share page / share_id → local file. Mints a fresh task_id then downloads immediately."""
        direct = self.share_direct(share, password=password, paths=paths)
        source_name = direct.get("name") or (direct.get("paths") or [None])[0]
        try:
            saved = self.client.download_url_to_file(
                direct["url"],
                dest,
                dry_run=dry_run,
                source_name=source_name,
            )
        except ApiError as exc:
            if exc.status_code != 401 or dry_run:
                raise
            direct = self.share_direct(share, password=password, paths=paths)
            saved = self.client.download_url_to_file(direct["url"], dest, source_name=source_name)
        return {**saved, "share_id": direct.get("share_id"), "direct": direct}

    def share_file(
        self,
        nas_path: str,
        *,
        expiration_days: int = 0,
        password: str | None = None,
        direct: bool = True,
    ) -> dict[str, Any]:
        created = self.share([nas_path], expiration_days=expiration_days, password=password)
        public_id = created.get("share_id") or created.get("id")
        result = {"share": created, "link": created.get("link"), "share_id": public_id, "id": created.get("id")}
        if direct and public_id is not None:
            result["direct"] = self.share_direct(str(public_id), password=password or "", paths=[nas_path])
        return result

    def _wait_download(self, added: dict[str, Any], save_dir: str, timeout: int) -> str:
        task_name = str(added.get("task_name") or "")
        deadline = time.time() + timeout

        def _items(payload: dict[str, Any]) -> list:
            data = payload.get("data") or payload
            if isinstance(data, list):
                return data
            if isinstance(data, dict):
                for key in ("result", "list", "tasks"):
                    value = data.get(key)
                    if isinstance(value, list):
                        return value
            return []

        while time.time() < deadline:
            self.client.list_downloads()
            done = _items(self.client.list_completed_downloads())
            match = [item for item in done if task_name and task_name in str(item)]
            if match:
                item = match[0]
                folder = item.get("save_dir") or item.get("savePath") or save_dir
                name = item.get("download_file_name") or item.get("fileName") or item.get("task_name")
                if isinstance(folder, str) and name and not str(folder).endswith(str(name)):
                    return f"{str(folder).rstrip('/')}/{name}"
                return str(folder)
            time.sleep(3)
        raise RuntimeError(f"download center did not finish within {timeout}s")
