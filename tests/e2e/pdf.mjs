const ws = new WebSocket(process.argv[2]); let n=0; const pend=new Map();
ws.onmessage=m=>{const d=JSON.parse(m.data); if(d.id&&pend.has(d.id)){pend.get(d.id)(d);pend.delete(d.id);}};
const call=(method,params={})=>new Promise(r=>{const id='p'+(++n);pend.set(id,r);ws.send(JSON.stringify({id,method,params}));});
ws.onopen=async()=>{const nb=(await call('notebook.create',{title:'pdf'})).result;
 const a=await call('source.add_files',{notebook_id:nb.id,paths:[process.argv[3]]}); console.log('add_files:', a.error?a.error.code:'ok');
 for(let i=0;i<40;i++){await new Promise(r=>setTimeout(r,1000)); const s=(await call('source.list',{notebook_id:nb.id})).result.sources[0];
   if(i%5==4||['READY','FAILED'].includes(s.status)){console.log(`t=${i+1}s status=${s.status} err=${s.error_code||''} ${s.error_detail||''}`);} if(['READY','FAILED'].includes(s.status))break;}
 process.exit(0);};
