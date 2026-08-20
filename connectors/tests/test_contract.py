"""Contract tests do adapter contra o mcp_server real v1.1.9."""

from __future__ import annotations

import os
import tempfile
import unittest
from pathlib import Path

from connectors.mcp_client import (
    McpClient,
    McpClientConfig,
    MemoryConnector,
    ScopeIdentity,
    SingleWriterError,
)


def _server_command() -> list[str] | None:
    configured = os.environ.get("NEURAL_SGDB_MCP_BIN")
    if configured:
        return [configured]
    suffix = ".exe" if os.name == "nt" else ""
    candidates = []
    cargo_target = os.environ.get("CARGO_TARGET_DIR")
    if cargo_target:
        candidates.append(Path(cargo_target) / f"release/examples/mcp_server{suffix}")
    candidates.extend((
        Path(f"target/release/examples/mcp_server{suffix}"),
        Path(f"target/debug/examples/mcp_server{suffix}"),
        Path(f".nsgdb/bin/mcp_server{suffix}"),
    ))
    for candidate in candidates:
        if candidate.exists():
            return [str(candidate.resolve())]
    return None


class ContractTest(unittest.TestCase):
    """Exercita handshake, quatro tools, scope e exclusão single-writer."""

    @classmethod
    def setUpClass(cls) -> None:
        command = _server_command()
        if command is None:
            raise unittest.SkipTest(
                "mcp_server ausente; rode cargo build --release --example mcp_server"
            )
        cls.tempdir = tempfile.TemporaryDirectory(prefix="nsgdb-connector-")
        cls.db_path = Path(cls.tempdir.name) / "contract.db"
        cls.config = McpClientConfig(command=command, db_path=cls.db_path)
        cls.client = McpClient(cls.config).start()

    @classmethod
    def tearDownClass(cls) -> None:
        cls.client.close()
        cls.tempdir.cleanup()

    def test_store_recall_forget_lexical_scoped(self) -> None:
        memory = MemoryConnector(
            self.client,
            ScopeIdentity("tenant-a", "agent-a", "workspace-a"),
            host="hermes",
            session_id="contract-session",
        )
        stored = memory.store(
            "preferência contratual única: editor solarized amber",
            "preference",
        )
        key = stored.get("storage_key")
        self.assertIsInstance(key, str)
        self.assertTrue(key.startswith("md/L3/"))

        hits = memory.recall("editor solarized amber")
        self.assertTrue(any(hit.get("key") == key for hit in hits))
        self.assertTrue(all(hit.get("path") == "lexical" for hit in hits))

        memory.forget(key)
        archived_hits = memory.recall("editor solarized amber")
        self.assertFalse(any(hit.get("key") == key for hit in archived_hits))

    def test_scope_isolation(self) -> None:
        first = MemoryConnector(
            self.client,
            ScopeIdentity("tenant-a", "agent-a", "workspace-one"),
            host="hermes",
            session_id="scope-one",
        )
        second = MemoryConnector(
            self.client,
            ScopeIdentity("tenant-a", "agent-a", "workspace-two"),
            host="hermes",
            session_id="scope-two",
        )
        stored = first.store("decisão isolada aurora-771", "decision")
        key = stored["storage_key"]
        self.assertTrue(any(hit.get("key") == key for hit in first.recall("aurora-771")))
        self.assertFalse(any(hit.get("key") == key for hit in second.recall("aurora-771")))

    def test_health_and_single_writer_lock(self) -> None:
        memory = MemoryConnector(
            self.client,
            ScopeIdentity("tenant-a", "agent-a", "workspace-a"),
            host="hermes",
            session_id="health",
        )
        health = memory.health()
        self.assertFalse(health.is_error)
        self.assertIn("backend", health.text)

        contender = McpClient(self.config)
        with self.assertRaises(SingleWriterError):
            contender.start()

    def test_auto_capture_defaults_off(self) -> None:
        memory = MemoryConnector(
            self.client,
            ScopeIdentity("tenant-a", "agent-a", "workspace-a"),
            host="hermes",
            session_id="capture",
        )
        self.assertIsNone(
            memory.auto_capture_turn(
                "eu prefiro respostas curtas",
                "entendido",
            )
        )


if __name__ == "__main__":
    unittest.main()
