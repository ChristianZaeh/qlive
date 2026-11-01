// Optional DOM integration test: npm install --prefix target/comment-dom jsdom
const {JSDOM}=require('../../target/comment-dom/node_modules/jsdom');
const fs=require('node:fs'),assert=require('node:assert/strict');
const tick=()=>new Promise(r=>setTimeout(r,0));
(async()=>{
 const dom=new JSDOM('<head><meta name="qlive-comment-token" content="fixture"></head><body><main><p>Chosen paragraph.</p></main></body>',{url:'http://localhost',runScripts:'outside-only'});
 const w=dom.window;w.ResizeObserver=class{observe(){}};let comments=[],posted;
 w.crypto.randomUUID=require('node:crypto').randomUUID;
 w.fetch=async(_url,options)=>{
  if(options.method==='POST'){
   posted=JSON.parse(options.body);
   if(posted.action==='resolve')comments[0].resolved=posted.resolved;
   else comments.push({...posted,id:'fixture',run_status:'note',resolved:false});
  }
  return {ok:true,json:async()=>({comments,sources:[]})};
 };
 w.eval(fs.readFileSync('crates/qlive-server/assets/comments.js','utf8'));await tick();
 w.document.querySelector('#qlive-comment-toggle').click();await tick();
 w.document.querySelector('main p').click();
 const input=w.document.querySelector('textarea');input.value='Explain <script>alert(1)</script>';
 const newline=new w.KeyboardEvent('keydown',{key:'Enter',shiftKey:true,cancelable:true});input.dispatchEvent(newline);assert.equal(newline.defaultPrevented,false);
 const enter=new w.KeyboardEvent('keydown',{key:'Enter',cancelable:true});input.dispatchEvent(enter);await tick();await tick();
 assert.equal(enter.defaultPrevented,true);assert.equal(posted.anchor.quote,'Chosen paragraph.');assert.equal(comments.length,1);
 assert.equal(input.value,'');assert.equal(w.document.querySelectorAll('script').length,0);assert.equal(w.document.querySelectorAll('.qlive-comment-pin').length,1);
 const article=w.document.querySelector('article');
 const pin=w.document.querySelector('.qlive-comment-pin');
 w.dispatchEvent(new w.Event('qlive-comments-changed'));await tick();
 assert.equal(w.document.querySelector('article'),article,'unchanged polling must preserve focused comment controls');
 assert.equal(w.document.querySelector('.qlive-comment-pin'),pin,'unchanged polling must preserve pin controls');
 w.document.querySelector('article button').click();await tick();assert.equal(comments[0].resolved,true);
 dom.window.close();console.log('PASS: click anchor, Enter submission, Shift+Enter, safe text rendering, pin and resolve');
})().catch(e=>{console.error(e);process.exitCode=1;});
