// Agent protocols are exercised with fake local CLIs; no paid agent is invoked.
const fs=require('node:fs/promises'),path=require('node:path'),os=require('node:os'),net=require('node:net'),assert=require('node:assert/strict');
const {spawn}=require('node:child_process');
const sleep=ms=>new Promise(r=>setTimeout(r,ms));
async function until(fn){for(let i=0;i<600;i++){const value=await fn();if(value)return value;await sleep(100);}throw Error('Timed out');}
(async()=>{
 const root=await fs.mkdtemp(path.join(os.tmpdir(),'qlive-agent-smoke-')),doc=path.join(root,'test.qmd');
 const saved='---\ntitle: Comments\n---\n\nSAVED TEXT\n',live=saved.replace('SAVED TEXT','UNSAVED TEXT');await fs.writeFile(doc,saved);
 const fake=path.join(root,'fake.js');await fs.writeFile(fake,`const fs=require('fs');let text='';process.stdin.on('data',c=>text+=c);process.stdin.on('end',()=>{const match=text.match(/listed in (.+sources.json)\\. Read/);const source=JSON.parse(fs.readFileSync(match[1],'utf8'))[0];const answer={summary:'Improved by fixture',edit:{file:source.file,new_text:source.before.replace('UNSAVED TEXT','AGENT TEXT')}};const i=process.argv.indexOf('--output-last-message');if(i>=0){fs.writeFileSync(process.argv[i+1],JSON.stringify(answer));console.log('{}');}else console.log(JSON.stringify({structured_output:answer}));});`);
 const server=net.createServer();await new Promise(r=>server.listen(0,'127.0.0.1',r));const port=server.address().port;await new Promise(r=>server.close(r));
 const url=`http://127.0.0.1:${port}`,token='agent-test-token';let logs='',child;
 const launch=()=>{logs='';child=spawn(path.resolve(process.argv[2]),['live',doc,'--port',String(port)],{windowsHide:true,env:{...process.env,QLIVE_EDITOR_TOKEN:token,QLIVE_CODEX_EXECUTABLE:process.execPath,QLIVE_CLAUDE_EXECUTABLE:process.execPath,QLIVE_CODEX_PREFIX_ARGS:JSON.stringify([fake]),QLIVE_CLAUDE_PREFIX_ARGS:JSON.stringify([fake])}});child.stdout.on('data',c=>logs+=c);child.stderr.on('data',c=>logs+=c);};
 const stop=async()=>{if(child.exitCode!==null)return;const done=new Promise(r=>child.once('exit',r));if(process.platform==='win32')spawn('taskkill.exe',['/PID',String(child.pid),'/T','/F'],{windowsHide:true});else child.kill();await done;};
 let commentToken;
 const api=(method,body,auth=commentToken)=>fetch(url+'/qlive/comments',{method,headers:{'x-qlive-comment-token':auth,'content-type':'application/json'},body:body&&JSON.stringify(body)});
 const agent=(method,id,body,auth=token)=>fetch(url+'/qlive/agent-result'+(id?'?id='+id:''),{method,headers:{authorization:'Bearer '+auth,'content-type':'application/json'},body:body&&JSON.stringify(body)});
 const start=async()=>{launch();await until(()=>logs.includes(url));const html=await until(async()=>{try{return await(await fetch(url)).text();}catch{return false;}});commentToken=html.match(/name="qlive-comment-token" content="([^"]+)"/)[1];};
 try{
 await start();assert.equal((await api('GET',undefined,'bad')).status,401);
 assert.equal((await fetch(url+'/qlive/buffer',{method:'POST',headers:{authorization:'Bearer '+token,'content-type':'application/json'},body:JSON.stringify({path:doc,text:live,dirty:true})})).status,200);
 const note={client_id:'note-1',message:'Explain <script>this</script>',anchor:{quote:'UNSAVED TEXT',tag:'p'}};
 const n=await(await api('POST',note)).json();assert.equal(n.comment.run_status,'note');assert.equal((await(await api('POST',note)).json()).comment.id,n.comment.id);
 for(const provider of ['codex','claude']){
 const res=await api('POST',{...note,client_id:provider,message:'@'+provider+' improve this'});const data=await res.json();assert.equal(res.status,200,JSON.stringify(data));const id=data.comment.id;
 await until(async()=>{const all=await(await api('GET')).json();const c=all.comments.find(c=>c.id===id);if(c.run_status==='failed')throw Error(c.reply);return c.run_status==='ready';});
 assert.equal((await agent('GET',id,undefined,'bad')).status,401);
 const proposal=await(await agent('GET',id)).json();assert.equal(proposal.before,live);assert.match(proposal.after,/AGENT TEXT/);
 assert.equal((await agent('POST',null,{id,status:provider==='claude'?'conflict':'applied',message:'fixture checked'})).status,200);
 }
 assert.equal(await fs.readFile(doc,'utf8'),saved);
 assert.match(await fs.readFile(path.join(root,'.qlive','agent-inbox.md'),'utf8'),/@claude/);
 await stop();await start();assert.equal((await(await api('GET')).json()).comments.length,3);
 assert.equal((await api('POST',{action:'resolve',id:n.comment.id,resolved:true})).status,200);
 console.log('PASS: both provider protocols, unsaved snapshots, auth, deduplication, acknowledgements, persistence; source unchanged');
 }catch(e){console.error(logs);throw e;}finally{await stop();await fs.rm(root,{recursive:true,force:true});}
})().catch(e=>{console.error(e);process.exitCode=1;});
