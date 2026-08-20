"""Cliente compartilhado dos adapters neural-sgdb via MCP/stdio."""

from .client import (
    McpClient,
    McpClientConfig,
    McpError,
    SingleWriterError,
    ToolResult,
)
from .contract import (
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
    "MemoryConnector",
    "RecallPolicy",
    "ScopeIdentity",
    "build_entities",
]
