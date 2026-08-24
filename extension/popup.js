const q = document.getElementById('q');
const hitsEl = document.getElementById('hits');
const countEl = document.getElementById('count');

function render(hits) {
  hitsEl.innerHTML = hits.length ? hits.map(h => `<div class="hit"><b>${h.key}</b><br>${h.text}</div>`).join('') : '<div class="muted">nada encontrado</div>';
}

document.getElementById('search').onclick = () => {
  chrome.runtime.sendMessage({ type: "recall", q: q.value }, render);
};
q.onkeydown = (e) => { if (e.key === 'Enter') document.getElementById('search').click(); };

chrome.runtime.sendMessage({ type: "stats" }, ({ count }) => {
  countEl.textContent = `${count || 0} memórias`;
});
