"""Cliente compartilhado dos adapters neural-sgdb via MCP/stdio."""

from .client import (
    McpClient,
    McpClientConfig,
    McpError,
    SingleWriterError,
    ToolResult,
)
from .contract import (
    MEMORY_KINDS,
    HOSTS,
    SENSITIVE_MEMORY_CATEGORIES,
    THIRD_PARTY_SUBJECT_TYPES,
    MemoryConnector,
    RecallPolicy,
    ScopeIdentity,
    build_entities,
)

__all__ = [
    "McpClient",
    "McpClientConfig",
    "McpError",
    "SingleWriterError",
    "ToolResult",
    "MEMORY_KINDS",
    "HOSTS",
    "SENSITIVE_MEMORY_CATEGORIES",
    "THIRD_PARTY_SUBJECT_TYPES",
    "MemoryConnector",
    "RecallPolicy",
    "ScopeIdentity",
    "build_entities",
]
