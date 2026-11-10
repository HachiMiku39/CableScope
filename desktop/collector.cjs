'use strict';
const {spawn}=require('node:child_process');
const http=require('node:http');

function localURL(line){
  if(!/^http:\/\/127\.0\.0\.1:\d{1,5}\/[A-Za-z0-9_-]{24,}\/$/.test(line))return null;
  try{const url=new URL(line);return Number(url.port)>0&&Number(url.port)<=65535?url.href:null;}catch{return null;}
}
function quitCollector(base){
  return new Promise(resolve=>{
    const request=http.request(new URL('api/quit',base),{method:'POST',headers:{'Content-Type':'application/json','Content-Length':'2','Connection':'close'},timeout:2000},response=>{response.resume();response.on('end',resolve);});
    request.on('error',resolve);request.on('timeout',()=>request.destroy());request.end('{}');
  });
}
function startCollector(command,args,options={}){
  const child=spawn(command,args,{windowsHide:true,stdio:['ignore','pipe','pipe'],shell:false});
  let base,buffer='',ended=false,stopping=false,stopPromise;
  const exited=new Promise(resolve=>child.once('exit',()=>{ended=true;resolve();}));
  const ready=new Promise((resolve,reject)=>{
    const timer=setTimeout(()=>{child.kill();reject(Error('本机采集程序启动超时。'));},options.timeout||30000);
    const fail=()=>{clearTimeout(timer);reject(Error('本机采集程序无法启动，请重新打开客户端。'));};
    child.once('error',fail);
    child.once('exit',code=>{if(!base)fail();else if(!stopping)options.onExit?.(code);});
    child.stdout.setEncoding('utf8');
    child.stdout.on('data',text=>{
      buffer+=text;
      if(buffer.length>8192){buffer='';return;}
      const lines=buffer.split(/\r?\n/);buffer=lines.pop();
      for(const line of lines){const url=localURL(line.trim());if(url&&!base){base=url;clearTimeout(timer);resolve(url);}}
    });
    // Drain output without logging tokens, hardware names or paths.
    child.stderr.resume();
  });
  async function stop(){
    if(stopPromise)return stopPromise;
    stopping=true;
    stopPromise=(async()=>{
      if(ended)return;
      if(base)await quitCollector(base);
      let timer;
      await Promise.race([exited,new Promise(resolve=>{timer=setTimeout(resolve,3500);})]);
      clearTimeout(timer);
      if(!ended)child.kill();
    })();
    return stopPromise;
  }
  return {ready,stop,child};
}
module.exports={localURL,startCollector};
