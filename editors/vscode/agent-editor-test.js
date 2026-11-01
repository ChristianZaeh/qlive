// Verify version-checked editor application without opening or modifying the user's editor.
const vm=require('node:vm'),fs=require('node:fs'),assert=require('node:assert/strict');
const source=fs.readFileSync(__dirname+'/extension.js','utf8');
async function run(newer,changesWhileOpening=false){
 let text=newer?'newer typing':'before',edits=0,ack;
 const doc={getText:()=>text,positionAt:offset=>offset};
 const vscode={Uri:{file:p=>p},ViewColumn:{One:1},Range:class{},workspace:{openTextDocument:async()=>doc},window:{showTextDocument:async()=>{if(changesWhileOpening)text='newer typing';return{edit:async fn=>{fn({replace:(_range,after)=>{text=after;edits++;}});return true;}};}}};
 const context={require:name=>name==='vscode'?vscode:require(name),module:{exports:{}},process,Buffer,console,setTimeout,clearTimeout};vm.createContext(context);
 vm.runInContext(source+`;session={files:new Set([fileKey('test.qmd')]),output:{appendLine(){}}};agentHttp=async(s,method,id,body)=>method==='GET'?{file:'test.qmd',before:'before',after:'after',summary:'done'}:capture(body);globalThis.test=()=>applyAgent(session,'id');`,context);
 context.capture=body=>{ack=body;};await context.test();return{text,edits,ack};
}
(async()=>{
 assert.equal((await run(false)).text,'after');assert.equal((await run(false)).ack.status,'applied');
 for(const result of [await run(true),await run(false,true)]){assert.equal(result.text,'newer typing');assert.equal(result.edits,0);assert.equal(result.ack.status,'conflict');}
 console.log('PASS: editor applies matching snapshot and preserves newer typing before/after opening');
})().catch(e=>{console.error(e);process.exitCode=1;});
