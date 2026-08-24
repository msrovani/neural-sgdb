// nsgdb extension — auto-captura verbatim (scope:"browser") em WasmStorage (hoje RAM, amanhã IndexedDB)
// Sem nuvem, sem API key. Cada visita vira L3 léxico: md/L3/<url-hash> com trecho 2k.

const DB_NS = "nsgdb-browser";
const MAX_SNIPPET = 2000;

// Stub RAM hoje — quando crates/nsgdb-wasm feature=wasm, trocar por:
// import init, { WasmStorage } from './pkg/nsgdb_wasm.js'; await init(); const storage = new WasmStorage(DB_NS);
let mem = new Map(); // key -> {title, url, snippet, ts}

function putBrowserMemory(url, title, snippet) {
  const k = `md/L3/${btoa(url).slice(0, 32)}`;
  mem.set(k, { title, url, snippet: snippet.slice(0, MAX_SNIPPET), ts: Date.now() });
  chrome.storage.local.set({ [k]: { title, url, snippet: snippet.slice(0, MAX_SNIPPET) } });
}

chrome.history.onVisited.addListener(({ url, title }) => {
  if (!url || url.startsWith("chrome://")) return;
  putBrowserMemory(url, title || url, "");
});

chrome.tabs.onUpdated.addListener((tabId, changeInfo, tab) => {
  if (changeInfo.status !== 'complete' || !tab.url || tab.url.startsWith("chrome://")) return;
  chrome.scripting.executeScript({
    target: { tabId },
    func: () => document.body.innerText.slice(0, 2000)
  }, (results) => {
    const snippet = results && results[0] && results[0].result ? results[0].result : "";
    putBrowserMemory(tab.url, tab.title || tab.url, snippet);
  });
});

chrome.runtime.onMessage.addListener((msg, _sender, reply) => {
  if (msg.type === "recall") {
    const q = (msg.q || "").toLowerCase();
    const hits = [];
    for (const [k, v] of mem) {
      const hay = `${v.title} ${v.url} ${v.snippet}`.toLowerCase();
      if (hay.includes(q)) hits.push({ key: k, text: `${v.title} | ${v.url}`, dist: 0, path: "lexical" });
      if (hits.length >= 5) break;
    }
    // fallback para chrome.storage se mem vazia (service worker reiniciado)
    if (hits.length === 0) {
      chrome.storage.local.get(null, (items) => {
        const more = [];
        for (const [k, v] of Object.entries(items)) {
          if (!k.startsWith("md/L3/")) continue;
          const hay = `${v.title} ${v.url} ${v.snippet}`.toLowerCase();
          if (hay.includes(q)) more.push({ key: k, text: `${v.title} | ${v.url}`, dist: 0 });
          if (more.length >= 5) break;
        }
        reply(more);
      });
      return true;
    }
    reply(hits);
  }
  if (msg.type === "stats") {
    reply({ count: mem.size });
  }
});
