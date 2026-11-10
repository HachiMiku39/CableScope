"use strict";
const $ = id => document.getElementById(id);
const esc = value => String(value ?? "未知").replace(/[&<>"']/g, x => ({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"}[x]));
const fmt = (n, digits=1) => typeof n === "number" && Number.isFinite(n) ? n.toFixed(digits).replace(/\.0$/, "") : "—";
const date = at => at ? new Date(at).toLocaleString("zh-CN", {hour12:false}) : "—";
const val = (n, unit) => fmt(n) + (n == null ? "" : " " + unit);
let latest, closed=false, polling=false;
const request=(path,options)=>window.CableLink.request(path,options);
function toast(message) { $("toast").textContent=message; $("toast").classList.add("show"); setTimeout(()=>$("toast").classList.remove("show"),3500); }
function graph(rows) {
  const canvas=$("chart"), box=canvas.getBoundingClientRect(), dpr=window.devicePixelRatio||1;
  canvas.width=box.width*dpr; canvas.height=box.height*dpr;
  const ctx=canvas.getContext("2d"); ctx.scale(dpr,dpr);
  const w=box.width, h=box.height, left=36, right=12, top=15, bottom=25;
  const series=[{key:"input_w",color:"#6261df"},{key:"battery_net_w",color:"#25a994"}];
  const values=rows.flatMap(r=>series.map(s=>r[s.key])).filter(n=>typeof n==="number" && Number.isFinite(n));
  $("chart-empty").hidden=values.length>0;
  const lo=Math.min(0,...values), hi=Math.max(10,...values), range=hi-lo||1;
  const y=n=>top+(hi-n)/range*(h-top-bottom);
  ctx.font="9px -apple-system, sans-serif";
  for(let i=0;i<=3;i++){const n=lo+range*i/3;ctx.strokeStyle="#eef0f6";ctx.beginPath();ctx.moveTo(left,y(n));ctx.lineTo(w-right,y(n));ctx.stroke();ctx.fillStyle="#9aa3b4";ctx.fillText(Math.round(n)+" W",0,y(n)+3);}
  if(!rows.length)return;
  const x=i=>left+(i/Math.max(1,rows.length-1))*(w-left-right);
  for(const s of series){ctx.strokeStyle=s.color;ctx.lineWidth=2;ctx.beginPath();let pen=false;
    rows.forEach((r,i)=>{const n=r[s.key];if(typeof n!=="number"||!Number.isFinite(n)){pen=false;return;}if(pen)ctx.lineTo(x(i),y(n));else ctx.moveTo(x(i),y(n));pen=true;});ctx.stroke();
    if(rows.length===1&&typeof rows[0][s.key]==="number"){ctx.fillStyle=s.color;ctx.beginPath();ctx.arc(x(0),y(rows[0][s.key]),3,0,Math.PI*2);ctx.fill();}}
  ctx.fillStyle="#9aa3b4";ctx.fillText(new Date(rows[0].at).toLocaleTimeString("zh-CN",{hour12:false}),left,h-5);
  ctx.textAlign="right";ctx.fillText(new Date(rows[rows.length-1].at).toLocaleTimeString("zh-CN",{hour12:false}),w-right,h-5);
}
function render(data) {
  latest=data;const s=data.snapshot,p=s.power||{};
  window.CableLink.showLocal(data.bridge);
  $("platform").textContent=s.platform+" / "+(s.loading?"首次读取中":date(s.at));
  $("status").textContent=s.loading?"正在读取":s.errors?.length?"部分数据不可用":"本机数据已更新";
  $("watts").innerHTML=fmt(p.input_w)+' <small>W</small>';
  $("rated").innerHTML=fmt(p.adapter_rated_w)+' <small>W</small>';
  $("battery").innerHTML=fmt(p.battery_net_w)+' <small>W</small>';
  $("power-caption").textContent=p.input_w==null?"系统未提供可用的输入功率":`${val(p.input_voltage_v,"V")} · ${val(p.input_current_a,"A")} · 系统遥测`;
  $("battery-caption").textContent=(p.battery_percent==null?"电量未知":fmt(p.battery_percent)+"% 电量")+" · 正充入 / 负放出";
  $("port-count").textContent=s.ports.length?s.ports.filter(p=>p.connected===true).length+" / "+s.ports.length:"—";
  $("power-source").textContent=p.source||"等待读取；未知值保持空白。";
  $("profiles").innerHTML=p.profiles?.length?p.profiles.map(x=>`<span class="chip">${esc(val(x.voltage_v,"V"))} / ${esc(val(x.current_a,"A"))}</span>`).join(""):'<span class="muted">系统未提供档位列表。</span>';
  $("diagnostics").innerHTML=(s.diagnostics||[]).map(n=>`<div class="note ${n.level==="warn"?"warn":""}"><b>${esc(n.title)}</b><p>${esc(n.text)}</p></div>`).join("")||'<p class="muted">正在读取系统状态…</p>';
  $("port-grid").innerHTML=s.ports.length?s.ports.map(p=>{const c=p.cable;return `<article class="panel"><div class="port-top"><h3>${esc(p.name)}</h3><span class="port-status ${p.connected===true?"":"disconnected"}">${p.connected===true?"● 已连接":p.connected===false?"○ 未连接":"状态未知"}</span></div><div class="protocol">${esc(p.transports?.join(" · ")||"无已知活动协议")}</div><dl><dt>E-Marker 声明速率</dt><dd>${esc(c?.speed||"未读取到")}</dd><dt>线缆声明电流</dt><dd>${esc(val(c?.current_a,"A"))}</dd><dt>线缆声明功率上限</dt><dd>${esc(val(c?.power_ceiling_w,"W"))}</dd><dt>插头方向 / 过流累计</dt><dd>${esc(p.orientation)} / ${esc(p.overcurrent_count??"未知")}</dd><dt>供应商 VID / 产品 PID</dt><dd>${c?esc([c.vendor_id,c.product_id].map(v=>v==null?"未知":"0x"+v.toString(16).padStart(4,"0")).join(" / ")):"—"}</dd></dl><details><summary>查看原始声明与引脚配置</summary><pre>${esc(JSON.stringify({vdo:c?.vdo_hex||[],pin_configuration:p.pin_configuration,plug_count:p.plug_count,liquid_detected:p.liquid_detected,source:p.source},null,2))}</pre></details><div class="footnote">${esc(c?.note||"未读取到 E-Marker，不代表线缆不支持高速或大功率。")}</div></article>`;}).join(""):'<article class="panel empty">'+(s.loading?'正在读取…':'此系统未公开可识别的 USB-C 物理端口。下方仍可查看 USB 设备；Windows 首版尚未读取 UCSI 的线缆身份。')+'</article>';
  const devices=(id,list,fn,empty)=>{$(id).innerHTML=list.length?list.map(fn).join(""):`<p class="empty">${empty}</p>`;};
  devices("display-list",s.displays,d=>`<div class="device"><b>${esc(d.name)}</b><p class="device-value">${esc(d.width)} × ${esc(d.height)} · ${d.refresh_hz==null?"刷新率未知":esc(fmt(d.refresh_hz))+" Hz"}</p><p>${esc(d.source)}</p></div>`,"未读取到当前显示模式；这不证明没有显示器连接。");
  devices("usb-list",s.devices,d=>`<div class="device"><b>${esc(d.name)}</b><p class="device-value">${esc(d.speed)}</p><p>${esc(d.location||d.vendor||"")} ${esc(d.source)}</p></div>`,"没有读取到 USB 设备。");
  devices("fabric-list",s.fabric,d=>`<div class="device"><b>${esc(d.name)}</b><p class="device-value">${esc(d.speed)}</p><p>${esc(d.mode)} · ${esc(d.path)}</p></div>`,"没有可用的 Thunderbolt / USB4 拓扑数据。");
  $("records").innerHTML=data.records.length?'<table><thead><tr><th>测试 / 时间</th><th>系统输入</th><th>备注</th></tr></thead><tbody>'+data.records.map(r=>`<tr><td>${esc(r.name)}<br><small>${esc(date(r.at))}</small></td><td>${esc(val(r.snapshot.power?.input_w,"W"))}</td><td>${esc(r.notes||"—")}</td></tr>`).join("")+"</tbody></table>":'<p class="empty">还没有测试记录。换一条线，用同一套设备测试，再保存快照。</p>';
  $("events").innerHTML=data.events.length?data.events.slice(0,10).map(e=>`<div class="device"><b>${esc(e.text)}</b><p>${esc(date(e.at))}</p></div>`).join(""):'<p class="empty">本次运行还没有观察到物理端口状态变化。Windows 首版不记录物理 USB-C 口插拔。</p>';
  $("limits").innerHTML=(s.limitations||[]).map(l=>`<li>${esc(l)}</li>`).join("");
  $("errors").innerHTML=(s.errors||[]).slice(0,20).map(e=>`<p class="error">${esc(e)}</p>`).join("");
  graph(data.history);
}
async function poll(){if(closed||polling||!window.CableLink.connected||window.CableLink.offline)return;polling=true;try{const r=await request("api/state");if(!r.ok)throw Error("读取失败");render(await r.json());}catch(e){$("status").textContent="本地服务未连接";}finally{polling=false;}}
$("save-open").onclick=()=>$("save-dialog").showModal();
$("save-cancel").onclick=()=>$("save-dialog").close();
$("save-form").onsubmit=async e=>{e.preventDefault();try{const r=await request("api/save",{method:"POST",headers:{"Content-Type":"application/json"},body:JSON.stringify({name:$("record-name").value,notes:$("record-notes").value})});const result=await r.json();if(!r.ok)throw Error(result.error||"保存失败");$("save-dialog").close();toast("测试快照已保存在本机");render(await (await request("api/state")).json());}catch(e){toast(e.message);}};
$("quit").onclick=async()=>{try{const r=await request("api/quit",{method:"POST",headers:{"Content-Type":"application/json"},body:"{}"});if(!r.ok)throw Error();closed=true;$("status").textContent="已退出";toast("CableScope 已停止采样，可以关闭此页");}catch(e){toast("退出失败，可在启动窗口按 Ctrl+C");}};
function download(blob,name){const url=URL.createObjectURL(blob),a=document.createElement("a");a.href=url;a.download=name;a.click();setTimeout(()=>URL.revokeObjectURL(url),1000);}
for(const [id,path,name] of [["export-json","api/report","CableScope-report.json"],["export-csv","api/history.csv","CableScope-power.csv"]])$(id).onclick=async()=>{try{if(window.CableLink.offline&&id==="export-json")return download(new Blob([JSON.stringify({app:"CableScope",version:"0.2.0",...latest},null,2)],{type:"application/json"}),name);const r=await request(path);if(!r.ok)throw Error("导出失败");download(await r.blob(),name);}catch(e){toast(e.message);}};
window.addEventListener("resize",()=>{if(latest)graph(latest.history);});
window.addEventListener("cablescope-connected",()=>{closed=false;poll();});
window.CableRender=render;
if(!window.CableLink.local){render({snapshot:{platform:"尚未连接采集程序",at:null,power:{},ports:[],devices:[],displays:[],fabric:[],errors:[],limitations:["连接本机采集程序后开始显示真实读数；也可导入 JSON 报告离线查看。"]},history:[],events:[],records:[]});$("status").textContent="未连接";}
setInterval(poll,3000);
poll();
