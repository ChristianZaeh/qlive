(() => {
  'use strict';
  const token = document.querySelector('meta[name="qlive-comment-token"]')?.content;
  if (!token) return;
  const style = document.createElement('style');
  style.textContent = `#qlive-comments{position:fixed;right:12px;top:58px;width:min(340px,90vw);max-height:80vh;overflow:auto;background:white;color:#20242a;border:1px solid #ccd1d8;border-radius:10px;box-shadow:0 8px 35px #0003;padding:16px;z-index:10020;font:14px/1.5 system-ui}#qlive-comments[hidden]{display:none}#qlive-comments textarea{box-sizing:border-box;width:100%;min-height:90px;font:inherit}#qlive-comments button,#qlive-comment-toggle{font:13px system-ui;padding:6px 10px;cursor:pointer}#qlive-comment-toggle{position:fixed;right:112px;top:12px;z-index:10021;background:white;color:#222;border:1px solid #bbc2cb;border-radius:6px}#qlive-comments article{border-top:1px solid #ddd;margin-top:12px;padding-top:12px;white-space:pre-wrap;overflow-wrap:anywhere}#qlive-comments small{display:block;color:#657080}#qlive-comments blockquote{margin:8px 0;padding-left:10px;border-left:3px solid #8498be;font-size:12px}#qlive-comment-pins{position:absolute;top:0;left:0;z-index:10010}.qlive-comment-pin{position:absolute;border:0;border-radius:50%;background:#455fc8;color:white;width:24px;height:24px;cursor:pointer}body.qlive-comment-picking main{cursor:crosshair}@media print{#qlive-comments,#qlive-comment-toggle,#qlive-comment-pins{display:none!important}}`;
  document.head.append(style);
  const el = (tag, text, parent) => { const n = document.createElement(tag); if (text !== undefined) n.textContent = text; parent?.append(n); return n; };
  const toggle = el('button', 'Comment'); toggle.id = 'qlive-comment-toggle'; document.body.append(toggle);
  const panel = el('aside'); panel.id = 'qlive-comments'; panel.hidden = true; panel.setAttribute('aria-label', 'Preview comments'); document.body.append(panel);
  const pins = el('div'); pins.id = 'qlive-comment-pins'; document.body.append(pins);
  el('strong', 'Preview comments', panel);
  const hint = el('p', 'Click a paragraph, figure or cell to attach a comment.', panel);
  const quote = el('blockquote', '', panel);
  const input = el('textarea', undefined, panel); input.placeholder = '@claude improve this explanation'; input.setAttribute('aria-label','Comment or agent request');
  el('small', 'Enter sends · Shift+Enter adds a line. @codex or @claude starts an agent; @agent uses your default.', panel);
  const send = el('button', 'Send', panel);
  const error = el('p', '', panel); error.setAttribute('role', 'status');
  const list = el('div', undefined, panel);
  let anchor, comments = [], sources = [], armed = false, submitting = false, clientId;
  const normalize = text => text.replace(/\s+/g, ' ').trim();
  async function request(method, body) {
    const response = await fetch('/qlive/comments', {method, headers:{'x-qlive-comment-token':token,'content-type':'application/json'},body:body && JSON.stringify(body)});
    const result = await response.json(); if (!response.ok) throw new Error(result.error || 'Comments unavailable'); return result;
  }
  function mode(value) { panel.hidden = !value; armed = value; document.body.classList.toggle('qlive-comment-picking', armed); toggle.textContent = value ? 'Close comments' : 'Comment'; if(value) void refresh(); }
  toggle.onclick = () => mode(panel.hidden);
  window.addEventListener('message', event => { if (event.source === window.parent && event.data?.type === 'qlive-comment-mode') mode(true); });
  document.addEventListener('click', event => {
    if (!armed || event.target.closest('#qlive-comments,#qlive-comment-toggle,#qlive-comment-pins,a,button,input,textarea,select,summary')) return;
    const target = event.target.closest('main p,main pre,main h1,main h2,main h3,main h4,main h5,main h6,main table,main figure,main img,main svg,main li,main blockquote');
    if (!target) return;
    event.preventDefault(); event.stopImmediatePropagation();
    const selection = window.getSelection();
    const selected = selection && target.contains(selection.anchorNode) && target.contains(selection.focusNode) ? selection.toString() : '';
    const text = normalize(selected || target.textContent || target.getAttribute('alt') || target.getAttribute('src') || 'Image').slice(0,1000);
    const tag = target.tagName.toLowerCase();
    const matches = [...document.querySelectorAll('main '+tag)].filter(n=>normalize(n.textContent || n.getAttribute('alt') || n.getAttribute('src') || 'Image').includes(text));
    anchor = {quote:text,tag,occurrence:Math.max(0,matches.indexOf(target))};
    const id = target.closest('[data-qlive-source]')?.getAttribute('data-qlive-source');
    if (id !== undefined && sources[Number(id)]) { anchor.source_id = Number(id); anchor.source_key = sources[Number(id)].key; }
    quote.textContent = text; hint.textContent = 'Comment attached to this preview selection.'; clientId = crypto.randomUUID(); input.focus();
  }, true);
  async function submit() {
    if (submitting) return;
    if (!anchor) { error.textContent = 'Click a preview location first.'; return; }
    if (!input.value.trim()) return;
    submitting = true; send.disabled = true; error.textContent = 'Sending…';
    try { await request('POST', {message:input.value,anchor,client_id:clientId}); input.value=''; clientId=crypto.randomUUID(); error.textContent='Saved to project inbox.'; await refresh(); }
    catch(e) { error.textContent=e.message; }
    finally { submitting=false;send.disabled=false; }
  }
  input.addEventListener('keydown',event=>{if(event.key==='Enter'&&!event.shiftKey&&!event.isComposing){event.preventDefault();void submit();}});
  send.onclick = ()=>void submit();
  function locate(comment) {
    const a = comment.anchor;
    let scope = document.querySelector('main');
    if (a.source) {
      const index = sources.findIndex(s => s.key === a.source.key && s.file === a.source.file);
      if(index<0) return;
      scope=document.querySelector(`[data-qlive-source="${index}"]`); if(!scope)return;
    }
    const matches = [...(scope?.querySelectorAll(a.tag) || [])].filter(n=>normalize(n.textContent || n.getAttribute('alt') || n.getAttribute('src') || 'Image').includes(a.quote));
    // Ambiguous matches after a change stay in the inbox rather than moving a pin to another paragraph.
    return matches.length===1 ? matches[0] : undefined;
  }
  function positionPins() {
    pins.replaceChildren();
    comments.filter(c=>!c.resolved).forEach((comment,i)=>{
      const target=locate(comment); if(!target)return;
      const rect=target.getBoundingClientRect();
      const pin=el('button',String(i+1),pins);pin.className='qlive-comment-pin';pin.title=comment.message;
      pin.style.left=Math.max(0,rect.right+window.scrollX-12)+'px';pin.style.top=(rect.top+window.scrollY)+'px';
      pin.onclick=()=>{mode(true);document.getElementById('comment-'+comment.id)?.scrollIntoView({block:'nearest'});};
    });
  }
  async function refresh() {
    try {
      const data=await request('GET'); comments=data.comments;sources=data.sources;
      list.replaceChildren();
      for(const comment of comments){
        const article=el('article',undefined,list);article.id='comment-'+comment.id;
        el('small',(comment.resolved?'Resolved · ':'')+comment.run_status+(locate(comment)?'':' · location changed'),article);
        el('blockquote',comment.anchor.quote,article);el('div',comment.message,article);
        if(comment.reply)el('p',comment.reply,article);
        const resolve=el('button',comment.resolved?'Reopen':'Resolve',article);
        resolve.onclick=async()=>{try{await request('POST',{action:'resolve',id:comment.id,resolved:!comment.resolved});await refresh();}catch(e){error.textContent=e.message;}};
      }
      positionPins();
    } catch(e){error.textContent=e.message;}
  }
  window.addEventListener('qlive-comments-changed',()=>void refresh());
  window.addEventListener('qlive-preview-updated',()=>void refresh());
  window.addEventListener('resize',positionPins);
  new ResizeObserver(positionPins).observe(document.body);
  setInterval(()=>{if(!panel.hidden)void refresh();},2500);
  void refresh();
})();
