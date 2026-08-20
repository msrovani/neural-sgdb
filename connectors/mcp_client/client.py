"""Transporte MCP newline-delimited com subprocesso e exclusão single-writer."""

from __future__ import annotations

import json
import os
import queue
import subprocess
import threading
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Mapping, Sequence

PROTOCOL_VERSION = "2025-11-25"
EXPECTED_TOOLS = frozenset({"remember", "recall", "health", "curate"})


class McpError(RuntimeError):
    """Falha de transporte, protocolo JSON-RPC ou tool MCP."""


class SingleWriterError(McpError):
    """Outro adapter já declarou propriedade exclusiva do arquivo de banco."""


@dataclass(frozen=True)
class McpClientConfig:
    """Configuração segura para iniciar um mcp_server dedicado."""

    command: Sequence[str]
    db_path: Path
    timeout_seconds: float = 10.0
    extra_env: Mapping[str, str] = field(default_factory=dict)
    client_name: str = "neural-sgdb-connector"
    client_version: str = "1.0"

    def __post_init__(self) -> None:
        if not self.command or any(not part for part in self.command):
            raise ValueError("command deve conter executável e argumentos não vazios")
        if self.timeout_seconds <= 0:
            raise ValueError("timeout_seconds deve ser positivo")


@dataclass(frozen=True)
class ToolResult:
    """Resposta normalizada de `tools/call`."""

    text: str
    structured: Mapping[str, Any]
    is_error: bool
    raw: Mapping[str, Any]


class _SingleWriterLock:
    """Lock cooperativo atômico; todos os adapters devem usar este protocolo."""

    def __init__(self, db_path: Path) -> None:
        self.path = Path(f"{db_path}.connector.lock")
        self._owned = False

    def acquire(self) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True)
        payload = json.dumps(
            {"pid": os.getpid(), "created_unix": int(time.time())},
            separators=(",", ":"),
        ).encode("utf-8")
        try:
            descriptor = os.open(
                self.path,
                os.O_CREAT | os.O_EXCL | os.O_WRONLY,
                0o600,
            )
        except FileExistsError as exc:
            try:
                owner = self.path.read_text(encoding="utf-8", errors="replace")
            except OSError:
                owner = "owner indisponível"
            raise SingleWriterError(
                f"banco já reservado por outro connector: {self.path} ({owner}). "
                "Remova o lock somente após confirmar que o processo proprietário encerrou."
            ) from exc
        try:
            os.write(descriptor, payload)
        finally:
            os.close(descriptor)
        self._owned = True

    def release(self) -> None:
        if not self._owned:
            return
        try:
            self.path.unlink(missing_ok=True)
        finally:
            self._owned = False


class McpClient:
    """Cliente síncrono MCP para um processo mcp_server de propriedade exclusiva."""

    def __init__(self, config: McpClientConfig) -> None:
        self.config = config
        self._lock = _SingleWriterLock(config.db_path)
        self._process: subprocess.Popen[str] | None = None
        self._responses: queue.Queue[Mapping[str, Any] | BaseException] = queue.Queue()
        self._reader: threading.Thread | None = None
        self._rpc_lock = threading.RLock()
        self._request_id = 0
        self.server_info: Mapping[str, Any] = {}

    def __enter__(self) -> McpClient:
        return self.start()

    def __exit__(self, *_: object) -> None:
        self.close()

    @property
    def is_running(self) -> bool:
        return self._process is not None and self._process.poll() is None

    def start(self) -> McpClient:
        if self.is_running:
            return self
        self._lock.acquire()
        try:
            self._responses = queue.Queue()
            self._request_id = 0
            environment = os.environ.copy()
            # O adapter é lexical-first e não herda DemoEmbedder do shell.
            environment.pop("NEURAL_SGDB_EMBEDDER", None)
            environment.update(self.config.extra_env)
            environment["NEURAL_SGDB_DB"] = str(self.config.db_path)
            self._process = subprocess.Popen(
                list(self.config.command),
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=None,
                text=True,
                encoding="utf-8",
                bufsize=1,
                env=environment,
                shell=False,
            )
            self._reader = threading.Thread(
                target=self._read_responses,
                name="neural-sgdb-mcp-reader",
                daemon=True,
            )
            self._reader.start()
            initialized = self.rpc(
                "initialize",
                {
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {
                        "name": self.config.client_name,
                        "version": self.config.client_version,
                    },
                },
            )
            result = initialized.get("result", {})
            if result.get("protocolVersion") != PROTOCOL_VERSION:
                raise McpError("mcp_server retornou protocolVersion incompatível")
            self.server_info = result.get("serverInfo", {})
            self.notify("notifications/initialized", {})
            self.assert_contract()
            return self
        except BaseException:
            self.close()
            raise

    def close(self) -> None:
        process, self._process = self._process, None
        try:
            if process is not None:
                if process.stdin is not None:
                    try:
                        process.stdin.close()
                    except OSError:
                        pass
                try:
                    process.wait(timeout=2.0)
                except subprocess.TimeoutExpired:
                    process.terminate()
                    try:
                        process.wait(timeout=2.0)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait(timeout=2.0)
                if process.stdout is not None:
                    process.stdout.close()
                if self._reader is not None:
                    self._reader.join(timeout=1.0)
                    self._reader = None
        finally:
            self._lock.release()

    def assert_contract(self) -> None:
        response = self.rpc("tools/list", {})
        tools = response.get("result", {}).get("tools", [])
        names = {tool.get("name") for tool in tools}
        if names != EXPECTED_TOOLS:
            raise McpError(
                f"contrato MCP incompatível: esperado {sorted(EXPECTED_TOOLS)}, "
                f"recebido {sorted(name for name in names if isinstance(name, str))}"
            )

    def rpc(self, method: str, params: Mapping[str, Any]) -> Mapping[str, Any]:
        with self._rpc_lock:
            return self._rpc_locked(method, params)

    def _rpc_locked(self, method: str, params: Mapping[str, Any]) -> Mapping[str, Any]:
        self._request_id += 1
        request_id = self._request_id
        self._write(
            {
                "jsonrpc": "2.0",
                "id": request_id,
                "method": method,
                "params": dict(params),
            }
        )
        deadline = time.monotonic() + self.config.timeout_seconds
        deferred: list[Mapping[str, Any]] = []
        try:
            while True:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise McpError(f"timeout aguardando resposta de {method}")
                try:
                    item = self._responses.get(timeout=remaining)
                except queue.Empty as exc:
                    raise McpError(f"timeout aguardando resposta de {method}") from exc
                if isinstance(item, BaseException):
                    raise McpError(f"reader MCP encerrou: {item}") from item
                if item.get("id") == request_id:
                    if "error" in item:
                        error = item["error"]
                        raise McpError(
                            f"JSON-RPC {error.get('code')}: {error.get('message')}"
                        )
                    return item
                deferred.append(item)
        finally:
            for item in deferred:
                self._responses.put(item)

    def notify(self, method: str, params: Mapping[str, Any]) -> None:
        self._write({"jsonrpc": "2.0", "method": method, "params": dict(params)})

    def call_tool(self, name: str, arguments: Mapping[str, Any]) -> ToolResult:
        response = self.rpc(
            "tools/call",
            {"name": name, "arguments": dict(arguments)},
        )
        result = response.get("result", {})
        content = result.get("content", [])
        text = next(
            (
                item.get("text", "")
                for item in content
                if item.get("type") == "text"
            ),
            "",
        )
        tool_result = ToolResult(
            text=text,
            structured=result.get("structuredContent", {}),
            is_error=bool(result.get("isError", False)),
            raw=result,
        )
        if tool_result.is_error:
            raise McpError(f"tool {name} falhou: {tool_result.text}")
        return tool_result

    def _write(self, message: Mapping[str, Any]) -> None:
        process = self._process
        if process is None or process.poll() is not None or process.stdin is None:
            raise McpError("mcp_server não está em execução")
        line = json.dumps(message, ensure_ascii=False, separators=(",", ":"))
        try:
            process.stdin.write(f"{line}\n")
            process.stdin.flush()
        except (BrokenPipeError, OSError) as exc:
            raise McpError("mcp_server fechou stdin") from exc

    def _read_responses(self) -> None:
        process = self._process
        if process is None or process.stdout is None:
            return
        try:
            for line in process.stdout:
                if not line.strip():
                    continue
                message = json.loads(line)
                if "id" in message:
                    self._responses.put(message)
            self._responses.put(EOFError("stdout do mcp_server foi fechado"))
        except BaseException as exc:
            self._responses.put(exc)
