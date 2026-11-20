// PDF import check: the source must reach READY with page-numbered chunks. Exits 1 otherwise.
const ws = new WebSocket(process.argv[2]); let n=0; const pend=new Map();
ws.onmessage=m=>{const d=JSON.parse(m.data); if(d.id&&pend.has(d.id)){pend.get(d.id)(d);pend.delete(d.id);}};
const call=(method,params={})=>new Promise(r=>{const id='p'+(++n);pend.set(id,r);ws.send(JSON.stringify({id,method,params}));});
ws.onopen=async()=>{const nb=(await call('notebook.create',{title:'pdf'})).result;
 const a=await call('source.add_files',{notebook_id:nb.id,paths:[process.argv[3]]}); console.log('add_files:', a.error?a.error.code:'ok');
 let src=null;
 for(let i=0;i<40;i++){await new Promise(r=>setTimeout(r,1000)); src=(await call('source.list',{notebook_id:nb.id})).result.sources[0];
   if(i%5==4||['READY','FAILED'].includes(src.status)){console.log(`t=${i+1}s status=${src.status} err=${src.error_code||''} ${src.error_detail||''}`);} if(['READY','FAILED'].includes(src.status))break;}
 let ok = src && src.status==='READY' && src.page_count===1;
 console.log(`${ok?'PASS':'FAIL'}  pdf import READY with page_count=1  — status=${src&&src.status} page_count=${src&&src.page_count}`);
 process.exit(ok?0:1);};
