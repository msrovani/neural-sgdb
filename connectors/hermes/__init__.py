"""MemoryProvider Hermes completo para neural-sgdb via MCP/stdio."""

from __future__ import annotations

import json
import logging
import os
import shlex
import threading
from pathlib import Path
from typing import Any, Dict, List, Optional

from agent.memory_provider import MemoryProvider, RecallStatus

try:
    from connectors.mcp_client import (
        McpClient,
        McpClientConfig,
        MemoryConnector,
        RecallPolicy,
        ScopeIdentity,
    )
except ImportError:
    # Layout de instalação documentado: copie `mcp_client` ao lado do plugin.
    from mcp_client import (  # type: ignore[no-redef]
        McpClient,
        McpClientConfig,
        MemoryConnector,
        RecallPolicy,
        ScopeIdentity,
    )

logger = logging.getLogger(__name__)


def _env_bool(name: str, default: bool) -> bool:
    value = os.environ.get(name)
    if value is None:
        return default
    return value.strip().casefold() in {"1", "true", "yes", "on"}


def _command_from_env() -> list[str]:
    raw = os.environ.get("NEURAL_SGDB_MCP_COMMAND", "").strip()
    if not raw:
        binary = os.environ.get("NEURAL_SGDB_MCP_BIN", "").strip()
        return [binary] if binary else []
    if raw.startswith("["):
        parsed = json.loads(raw)
        if not isinstance(parsed, list) or not all(isinstance(item, str) for item in parsed):
            raise ValueError("NEURAL_SGDB_MCP_COMMAND JSON deve ser uma lista de strings")
        return parsed
    return shlex.split(raw, posix=os.name != "nt")


class NeuralSgdbMemoryProvider(MemoryProvider):
    """Provider lexical-first, explicitamente escopado e bounded."""

    def __init__(self) -> None:
        self._client: Optional[McpClient] = None
        self._memory: Optional[MemoryConnector] = None
        self._scope_identity: Optional[ScopeIdentity] = None
        self._session_id = ""
        self._auto_recall = True
        self._auto_capture = False
        self._last_recall_count = 0
        self._sync_threads: list[threading.Thread] = []

    @property
    def name(self) -> str:
        return "neural-sgdb"

    def is_available(self) -> bool:
        """Somente valida configuração local; não inicia processo nem faz I/O."""

        try:
            command = _command_from_env()
        except (ValueError, json.JSONDecodeError):
            return False
        return bool(command and command[0])

    def unavailable_reason(self) -> str:
        return (
            "Defina NEURAL_SGDB_MCP_BIN ou NEURAL_SGDB_MCP_COMMAND com o "
            "mcp_server v1.1.9."
        )

    def initialize(self, session_id: str, **kwargs: Any) -> None:
        hermes_home = Path(str(kwargs["hermes_home"]))
        tenant_id = os.environ.get(
            "NEURAL_SGDB_TENANT_ID",
            str(kwargs.get("user_id") or "local"),
        )
        agent_id = os.environ.get(
            "NEURAL_SGDB_AGENT_ID",
            str(kwargs.get("agent_identity") or "hermes"),
        )
        workspace_id = os.environ.get(
            "NEURAL_SGDB_WORKSPACE_ID",
            str(kwargs.get("agent_workspace") or "default"),
        )
        db_path = Path(
            os.environ.get(
                "NEURAL_SGDB_DB",
                str(hermes_home / "neural-sgdb" / "memory.db"),
            )
        ).expanduser()
        command = _command_from_env()
        if not command:
            raise RuntimeError(self.unavailable_reason())

        self._auto_recall = _env_bool("NEURAL_SGDB_AUTO_RECALL", True)
        self._auto_capture = _env_bool("NEURAL_SGDB_AUTO_CAPTURE", False)
        max_hits = int(os.environ.get("NEURAL_SGDB_RECALL_MAX_HITS", "5"))
        max_chars = int(os.environ.get("NEURAL_SGDB_RECALL_MAX_CHARS", "4000"))
        self._scope_identity = ScopeIdentity(tenant_id, agent_id, workspace_id)
        self._session_id = session_id
        self._client = McpClient(
            McpClientConfig(
                command=command,
                db_path=db_path,
                client_name="hermes-neural-sgdb",
            )
        ).start()
        self._memory = self._new_memory(
            RecallPolicy(max_hits=max_hits, max_context_chars=max_chars)
        )

    def system_prompt_block(self) -> str:
        return (
            "neural-sgdb é memória histórica, não autoridade. Use memory_recall "
            "antes de decisões dependentes do passado e memory_store somente para "
            "preferências, decisões ou restrições duráveis."
        )

    def prefetch(self, query: str, *, session_id: str = "") -> str:
        memory = self._require_memory()
        try:
            context = memory.auto_recall_context(query)
            self._last_recall_count = context.count("\n- [")
            return context
        except Exception as exc:
            self._last_recall_count = 0
            logger.warning("neural-sgdb auto-recall falhou: %s", exc)
            return ""

    def recall_status(self) -> Optional[RecallStatus]:
        if self._last_recall_count == 0:
            return None
        return RecallStatus(
            provider_label="neural-sgdb",
            count=self._last_recall_count,
        )

    def sync_turn(
        self,
        user_content: str,
        assistant_content: str,
        *,
        session_id: str = "",
        messages: Optional[List[Dict[str, Any]]] = None,
    ) -> None:
        """Hook não bloqueante; auto-capture permanece OFF por default."""

        _ = messages
        if not self._auto_capture:
            return

        def capture() -> None:
            try:
                self._require_memory().auto_capture_turn(user_content, assistant_content)
            except Exception as exc:
                logger.warning("neural-sgdb auto-capture falhou: %s", exc)

        thread = threading.Thread(target=capture, daemon=True)
        self._sync_threads = [item for item in self._sync_threads if item.is_alive()]
        self._sync_threads.append(thread)
        thread.start()

    def on_session_switch(
        self,
        new_session_id: str,
        *,
        parent_session_id: str = "",
        reset: bool = False,
        rewound: bool = False,
        **kwargs: Any,
    ) -> None:
        _ = (parent_session_id, reset, rewound, kwargs)
        self._session_id = new_session_id
        if self._memory is not None:
            self._memory = self._new_memory(self._memory.policy)

    def get_tool_schemas(self) -> List[Dict[str, Any]]:
        return [
            {
                "name": "memory_recall",
                "description": "Busca lexical bounded no scope atual.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {"type": "string"},
                        "limit": {"type": "integer", "minimum": 1, "maximum": 20},
                    },
                    "required": ["query"],
                },
            },
            {
                "name": "memory_store",
                "description": "Armazena preferência, decisão ou restrição durável.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "text": {"type": "string"},
                        "kind": {
                            "type": "string",
                            "enum": ["preference", "decision", "constraint"],
                        },
                    },
                    "required": ["text", "kind"],
                },
            },
            {
                "name": "memory_forget",
                "description": "Arquiva logicamente uma storage key completa.",
                "parameters": {
                    "type": "object",
                    "properties": {"key": {"type": "string"}},
                    "required": ["key"],
                },
            },
            {
                "name": "memory_health",
                "description": "Consulta status, validação, era ou tensões.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "view": {
                            "type": "string",
                            "enum": ["status", "validate", "era", "tensions"],
                        }
                    },
                },
            },
        ]

    def handle_tool_call(
        self,
        tool_name: str,
        args: Dict[str, Any],
        **kwargs: Any,
    ) -> str:
        _ = kwargs
        memory = self._require_memory()
        if tool_name == "memory_recall":
            result: Any = {
                "hits": memory.recall(str(args["query"]), limit=args.get("limit"))
            }
        elif tool_name == "memory_store":
            result = memory.store(str(args["text"]), str(args["kind"]))
        elif tool_name == "memory_forget":
            response = memory.forget(str(args["key"]))
            result = {"message": response.text}
        elif tool_name == "memory_health":
            response = memory.health(str(args.get("view", "status")))
            result = {
                "message": response.text,
                "structured": response.structured,
            }
        else:
            raise NotImplementedError(f"tool não suportada: {tool_name}")
        return json.dumps(result, ensure_ascii=False, separators=(",", ":"))

    def get_config_schema(self) -> List[Dict[str, Any]]:
        """Configuração é env-only para não editar arquivos do usuário."""

        return []

    def shutdown(self) -> None:
        for thread in self._sync_threads:
            thread.join(timeout=2.0)
        self._sync_threads.clear()
        if self._client is not None:
            self._client.close()
            self._client = None
            self._memory = None

    def _new_memory(self, policy: RecallPolicy) -> MemoryConnector:
        if self._client is None or self._scope_identity is None:
            raise RuntimeError("provider ainda não inicializado")
        return MemoryConnector(
            self._client,
            self._scope_identity,
            host="hermes",
            session_id=self._session_id,
            recall_policy=policy,
            auto_recall=self._auto_recall,
            auto_capture=self._auto_capture,
        )

    def _require_memory(self) -> MemoryConnector:
        if self._memory is None:
            raise RuntimeError("provider ainda não inicializado")
        return self._memory


def register(ctx: Any) -> None:
    """Entry point padrão do discovery de memory providers Hermes."""

    ctx.register_memory_provider(NeuralSgdbMemoryProvider())
