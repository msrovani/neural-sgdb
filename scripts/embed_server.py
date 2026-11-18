#!/usr/bin/env python3
"""Local REAL embedder HTTP server for the LongMemEval hybrid benchmark.

Loads a sentence-transformers model (default `all-MiniLM-L6-v2`, 384d) and
serves the contract used by `examples/longmemeval.rs --embed-url`:

    POST /embed        {"text": "..."}           -> {"embedding": [...]}
    POST /embed_batch  {"texts": ["...", ...]}    -> {"embeddings": [[...], ...]}

No web framework: stdlib `http.server` (threaded) keeps the dependency surface
to sentence-transformers itself. Run:

    python scripts/embed_server.py [model_name] [port]
"""

import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

MODEL_NAME = sys.argv[1] if len(sys.argv) > 1 else "all-MiniLM-L6-v2"
PORT = int(sys.argv[2]) if len(sys.argv) > 2 else 8791

print(f"embed_server: loading {MODEL_NAME} ...", flush=True)
from sentence_transformers import SentenceTransformer  # noqa: E402

MODEL = SentenceTransformer(MODEL_NAME, device="cpu")
print("embed_server: model ready", flush=True)


def embed(texts):
    vecs = MODEL.encode(
        texts,
        normalize_embeddings=True,
        convert_to_numpy=True,
        batch_size=32,
    )
    return [[float(x) for x in v] for v in vecs]


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *args):
        pass

    def _send(self, code, obj):
        body = json.dumps(obj).encode("utf-8")
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        n = int(self.headers.get("Content-Length", 0))
        raw = self.rfile.read(n)
        try:
            req = json.loads(raw or b"{}")
        except Exception as e:  # noqa: BLE001
            return self._send(400, {"error": f"bad json: {e}"})
        try:
            if self.path == "/embed":
                return self._send(200, {"embedding": embed([req.get("text", "")])[0]})
            if self.path == "/embed_batch":
                texts = req.get("texts", [])
                if not texts:
                    return self._send(200, {"embeddings": []})
                return self._send(200, {"embeddings": embed(texts)})
            return self._send(404, {"error": f"unknown path {self.path}"})
        except Exception as e:  # noqa: BLE001
            return self._send(500, {"error": str(e)})


if __name__ == "__main__":
    srv = ThreadingHTTPServer(("127.0.0.1", PORT), Handler)
    print(f"embed_server: listening on http://127.0.0.1:{PORT}", flush=True)
    srv.serve_forever()
