"use strict";
(() => {
  const el=id=>document.getElementById(id);
  const local=(location.hostname==="127.0.0.1"||location.hostname==="localhost") && /^\/[A-Za-z0-9_-]{24,}\/$/.test(location.pathname);
  const link=window.CableLink={local,connected:local,offline:false,base:"",token:""};
  const message=text=>{el("connection-message").textContent=text;};
  const controls=()=>{
    const connected=link.connected&&!link.offline;
    for(const id of ["save-open","quit","export-csv"]) el(id).disabled=!connected;
    el("export-json").disabled=!connected&&!link.offline;
    el("disconnect").hidden=local||!connected;
    el("connect-button").disabled=connected;
    if(local&&link.offline){el("connect-form").hidden=false;el("connect-button").textContent="返回实时数据";el("pair-code").disabled=true;el("agent-port").disabled=true;}
    else if(local)el("connect-form").hidden=true;
  };
  link.request=async(path, options={})=>{
    if(!link.connected||link.offline) throw Error("请先连接本机采集程序");
    const headers=new Headers(options.headers||{});
    if(!local)headers.set("Authorization","Bearer "+link.token);
    const response=await fetch((local?"":link.base)+path,{...options,headers,cache:"no-store",credentials:"omit",referrerPolicy:"no-referrer",signal:AbortSignal.timeout(15000)});
    if(response.status===401){link.connected=false;link.token="";controls();message("配对已到期，请在本机窗口刷新配对码后重连。");}
    return response;
  };
  link.showLocal=info=>{
    if(!local||!info)return;
    el("local-code").textContent=info.code||"已使用";
    el("code-expiry").textContent="端口 "+new URL(info.base).port+" · "+(info.code?"有效期剩余 "+info.expires_in+" 秒 · 已连接 "+info.sessions+" 个网页":"如需再次配对，请刷新配对码");
    el("open-web").href=info.site;
  };
  const disconnect=async()=>{
    if(!local&&link.connected){try{await link.request("api/disconnect",{method:"POST",headers:{"Content-Type":"application/json"},body:"{}"});}catch{}}
    link.connected=false;link.token="";link.offline=false;controls();
    el("status").textContent="已断开";message("已断开网页连接；本机采集程序仍可独立运行。");
  };
  el("disconnect").onclick=disconnect;
  el("connect-form").onsubmit=async event=>{
    event.preventDefault();
    if(local){link.offline=false;controls();message("已恢复本机实时读数。");window.dispatchEvent(new Event("cablescope-connected"));return;}
    const port=Number(el("agent-port").value),code=el("pair-code").value.trim();
    if(!Number.isInteger(port)||port<1024||port>65535||!/^\d{8}$/.test(code))return message("请输入有效端口和八位数字配对码。");
    el("connect-button").disabled=true;message("正在连接本机；如浏览器询问，请允许访问本机采集程序。");
    try{
      const base=`http://127.0.0.1:${port}/bridge/`;
      const r=await fetch(base+"pair",{method:"POST",headers:{"Content-Type":"application/json"},body:JSON.stringify({code}),credentials:"omit",referrerPolicy:"no-referrer",signal:AbortSignal.timeout(15000)});
      const result=await r.json();if(!r.ok)throw Error(result.error||"配对失败");
      link.base=base;link.token=result.token;link.connected=true;link.offline=false;
      el("pair-code").value="";message("本机采集程序已连接。连接凭据仅保存在当前页面内存中，刷新页面后需要重新配对。");
      window.dispatchEvent(new Event("cablescope-connected"));
    }catch(error){message(error.name==="TypeError"||error.name==="TimeoutError"?"无法连接。请确认 0.2 或更新版采集程序已启动、端口正确，且浏览器允许访问本机。可使用本机离线界面继续查看。":error.message);}
    controls();
  };
  for(const [id,path] of [["new-code","new-code"],["revoke","revoke"]])el(id).onclick=async()=>{
    try{const r=await link.request("api/"+path,{method:"POST",headers:{"Content-Type":"application/json"},body:"{}"});if(!r.ok)throw Error("操作失败");message(id==="revoke"?"所有网页连接已撤销，配对码已更新。":"配对码已更新；原配对码失效。");window.dispatchEvent(new Event("cablescope-connected"));}catch(e){message(e.message);}
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
      if(!local&&link.connected)await disconnect();
      link.offline=true;controls();window.CableRender(report);el("status").textContent="离线报告";
      message("正在查看导入报告。文件只在当前浏览器处理；不会开始实时采样。");
    }catch(error){message(error.message);}
    event.target.value="";
  };
  if(local){el("connect-form").hidden=true;el("local-pairing").hidden=false;el("connection-title").textContent="本机采集程序正在运行";el("connection-description").textContent="此页可离线使用。也可以打开网页入口，输入下方配对码连接当前电脑。配对码五分钟有效，仅能使用一次。";}
  controls();
})();
