"use strict";
(() => {
  const el=id=>document.getElementById(id);
  const tauriOrigin=location.protocol==="cablescope:"||location.hostname==="cablescope.localhost";
  const tauri=tauriOrigin&&!!(window.__TAURI__&&window.__TAURI__.core&&typeof window.__TAURI__.core.invoke==="function");
  const link=window.CableLink={local:tauri,connected:tauri,offline:false,tauri};
  const message=text=>{el("connection-message").textContent=text;};
  const controls=()=>{
    const connected=link.connected&&!link.offline;
    for(const id of ["save-open","quit","export-csv"]) el(id).disabled=!connected;
    el("export-json").disabled=!connected&&!link.offline;
    el("connect-button").disabled=connected;
    if(tauri&&link.offline){el("connect-form").hidden=false;el("connect-button").textContent="返回实时数据";}
    else if(tauri)el("connect-form").hidden=true;
  };
  const compat=result=>{
    const status=Number(result&&result.status)||0,ok=status>=200&&status<300;
    const jsonValue=result&&Object.prototype.hasOwnProperty.call(result,"json")?result.json:undefined;
    const hasText=result&&typeof result.text==="string";
    const text=hasText?result.text:(jsonValue===undefined?"":JSON.stringify(jsonValue));
    const headers=result&&typeof result.headers==="object"&&result.headers!==null?result.headers:{};
    const type=typeof headers["content-type"]==="string"?headers["content-type"]:(hasText?"text/plain; charset=utf-8":"application/json; charset=utf-8");
    return {ok,status,headers:{get:name=>{const key=String(name).toLowerCase();return headers[key]??null;}},
      json:async()=>jsonValue!==undefined?jsonValue:JSON.parse(text||"{}"),
      text:async()=>text,blob:async()=>new Blob([text],{type})};
  };
  link.request=async(path, options={})=>{
    if(!link.connected||link.offline) throw Error("请先连接本机采集程序");
    const body=options.body===undefined?undefined:(typeof options.body==="string"?JSON.parse(options.body):options.body);
    const payload={path,method:options.method||"GET"};
    if(body!==undefined)payload.body=body;
    const response=await window.__TAURI__.core.invoke("api_request",{path,options:payload});
    if(path==="api/state"&&document.title==="CableScope · 线缆实验室")document.title="CableScope IPC 就绪";
    return compat(response);
  };
  el("connect-form").onsubmit=event=>{
    event.preventDefault();
    if(!tauri)return;
    link.offline=false;controls();message("已恢复本机实时读数。");window.dispatchEvent(new Event("cablescope-connected"));
  };
  el("import-report").onchange=async event=>{
    const file=event.target.files[0];if(!file)return;
    try{
      if(file.size>10*1024*1024)throw Error("报告超过 10 MB，请选择较小的 CableScope JSON 报告。");
      const report=JSON.parse(await file.text()),s=report.snapshot;
      if(!s||typeof s.platform!=="string"||!s.power||!Array.isArray(s.ports)||!Array.isArray(s.devices)||!Array.isArray(s.displays)||!Array.isArray(s.fabric))throw Error("报告格式不正确，请导入 CableScope 导出的 JSON。");
      for(const [key,max] of [["ports",100],["devices",500],["displays",64],["fabric",200],["diagnostics",100],["limitations",100],["errors",100]]){
        if(s[key]!==undefined&&!Array.isArray(s[key]))throw Error("报告字段格式不正确："+key);
        s[key]=(s[key]||[]).slice(0,max);
      }
      for(const p of s.ports)if(!p||typeof p.name!=="string"||(p.transports&&!Array.isArray(p.transports)))throw Error("端口信息格式不正确");
      report.history=(Array.isArray(report.history)?report.history:[]).slice(-120);
      report.records=(Array.isArray(report.records)?report.records:[]).filter(r=>r&&r.snapshot).slice(0,50);
      report.events=(Array.isArray(report.events)?report.events:[]).slice(0,100);
      link.offline=true;controls();window.CableRender(report);el("status").textContent="离线报告";
      message("正在查看导入报告。文件只在当前浏览器处理；不会开始实时采样。");
    }catch(error){message(error.message);}
    event.target.value="";
  };
  if(tauri){el("connect-form").hidden=true;el("connection-title").textContent="本机采集程序正在运行";el("connection-description").textContent="读数来自本机采集程序，只保存在这台电脑上。也可以导入已有报告离线查看。";}
  controls();
})();
