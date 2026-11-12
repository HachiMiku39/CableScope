// Exercise UI rendering as pure JavaScript, without browser or hardware access.
const fs=require('node:fs'),path=require('node:path'),vm=require('node:vm'),assert=require('node:assert/strict');
const root=path.resolve(__dirname,'..'),html=fs.readFileSync(path.join(root,'web/index.html'),'utf8');
const ctx=new Proxy({},{get:()=>()=>{}}),elements=new Map();
for(const match of html.matchAll(/id="([^"]+)"/g))elements.set(match[1],{value:'',innerHTML:'',textContent:'',open:false,hidden:false,disabled:false,
  getBoundingClientRect:()=>({width:700,height:195}),getContext:()=>ctx,showModal(){this.open=true;},close(){this.open=false;}});
elements.get('chart-mode').value='system';
const window={devicePixelRatio:1,addEventListener(){},dispatchEvent(){}},document={title:'CableScope · 线缆实验室',getElementById:id=>{assert(elements.has(id),'Unknown UI id: '+id);return elements.get(id);},createElement:()=>({click(){}})};
const stateFor=()=>({snapshot:{at:'2026-11-10T09:00:00Z',platform:'Windows',power:{input_w:25,battery_net_w:0},ports:[],devices:[],fabric:[],display_links:[],displays:[],diagnostics:[],errors:[],limitations:[],resistance:{status:'unavailable'}},records:[],events:[],history:[]});
const invocations=[],sandbox={window,document,location:{hostname:'localhost',protocol:'http:',pathname:'/'},Blob,URL,
  setTimeout:()=>0,setInterval:()=>0,Event:class{},console};
if(process.argv.includes('--tauri')){
  window.__TAURI__={core:{invoke:async(name,args)=>{invocations.push({name,args});
    const path=args.path;
    if(path==='api/state')return {status:200,json:stateFor(),headers:{'content-type':'application/json; charset=utf-8'}};
    if(path==='api/report')return {status:200,text:JSON.stringify({app:'CableScope',version:'0.4.0',...stateFor()}),headers:{'content-type':'application/json; charset=utf-8'}};
    if(path==='api/history.csv')return {status:200,text:'\ufeffat_UTC,input_w\r\n',headers:{'content-type':'text/csv; charset=utf-8'}};
    if(path==='api/save')return {status:200,json:{ok:true},headers:{'content-type':'application/json; charset=utf-8'}};
    if(path==='api/quit')return {status:200,json:{ok:true},headers:{'content-type':'application/json; charset=utf-8'}};
    return {status:404,json:{error:'资源不存在'},headers:{'content-type':'application/json; charset=utf-8'}};
  }}};
  sandbox.location={hostname:'cablescope.localhost',protocol:'cablescope:',pathname:'/index.html'};
}
vm.createContext(sandbox);
for(const file of ['connection.js','app.js'])vm.runInContext(fs.readFileSync(path.join(root,'web',file),'utf8'),sandbox,{filename:file});
const port={id:'p1',name:'<img src=x onerror=bad>',kind:'USB-C',connected:true,orientation:'方向 1',transports:['USB3'],
  pin_configuration:{tx1:1},pin_map:{rows:[[{id:'A1',signal:'GND',class:'fixed'}]],note:'逻辑路由'},
  output_power:{watts:5.2,voltage_v:5.2,current_a:1},contract:{watts:100,voltage_v:20,current_a:5},
  negotiation:{notes:['测试'],source_reported_w:140},cable:{speed:'USB · 10 Gbps',current_a:5,vdo_hex:[],vendor_id:4660,product_id:1}};
const snapshot={at:'2026-11-10T09:00:00Z',platform:'macOS',power:{input_w:25,battery_net_w:0},ports:[port],devices:[],fabric:[],
  display_links:[{name:'DP',active:true,lane_count:4,link_rate_description:'HBR3'}],
  displays:[{name:'Monitor',width:1920,height:1080,refresh_hz:60,highest_mode:{width:3840,height:2160,refresh_hz:60},verdict:'可选更高模式'}],
  diagnostics:[],errors:[],limitations:[],resistance:{status:'estimate',milliohms:99.2,sample_count:40,r_squared:.99}};
const state=process.argv[2]&&!process.argv[2].startsWith('--')?JSON.parse(fs.readFileSync(process.argv[2],'utf8')):{snapshot,
  records:[{name:'我的线',at:snapshot.at,notes:'人工记录',snapshot:{...snapshot,cable_info:{port_id:'p1',length_m:1,price_cny:39,softness:4}}}],
  events:[],history:[{at:snapshot.at,input_w:25,battery_net_w:0,port_power:[{id:'p1',name:'Port 1',output_w:5.2}]}]};
window.CableRender(state);
assert(elements.get('port-grid').innerHTML.includes('SMC 遥测'));
assert(elements.get('port-grid').innerHTML.includes('PD 合同'));
assert(elements.get('resistance-reading').innerHTML.includes('有效样本'));
if(!process.argv[2]||process.argv[2].startsWith('--')){
  assert(!elements.get('port-grid').innerHTML.includes('<img src=x'));
  assert(elements.get('port-grid').innerHTML.includes('&lt;img'));
  assert(elements.get('port-grid').innerHTML.includes('pin-diagram'));
  assert(elements.get('display-list').innerHTML.includes('3840'));
  assert(elements.get('records').innerHTML.includes('我的线'));
  elements.get('record-filter').value='不存在的线';elements.get('record-filter').oninput();
  assert(elements.get('records').innerHTML.includes('没有匹配'));
  assert(!elements.get('port-grid').innerHTML.includes('Windows 首版'));
  assert(elements.get('events').innerHTML.includes('本机未提供可读取的物理 USB-C 口插拔状态'));
  assert(!elements.get('port-grid').innerHTML.includes('本机未提供可读取的 UCSI 线缆身份数据'));
  const candidate={id:'candidate-1',name:'USB-C 候选 · Hub #2 Port 1',kind:'USB-C',connected:null,
    contract:null,output_power:null,input_power:null,cable:null,pin_configuration:{},pin_map:null,
    overcurrent_count:null,plug_count:null,liquid_detected:null,orientation:null,
    source:'Hub IOCTL 端口属性（候选，未确认机身物理口）',
    negotiation:{notes:['缺少完整链路上限，不生成线缆瓶颈结论'],source:'不同来源分列'}};
  const emptyState={...state,snapshot:{...snapshot,platform:'Windows',ports:[candidate],
    limitations:['本机固件未暴露可读取的 UCSI 设备节点或 WMI 数据面'],events:undefined},events:[]};
  window.CableRender(emptyState);
  const candidateHtml=elements.get('port-grid').innerHTML;
  assert(candidateHtml.includes('USB-C 候选 · Hub #2 Port 1'));
  assert(candidateHtml.includes('状态未知'));
  assert(!candidateHtml.includes('● 已连接'));
  assert(!candidateHtml.includes('undefined'));
  assert(!candidateHtml.includes('pin-diagram'));
  assert(candidateHtml.includes('缺少完整链路上限'));
  assert(elements.get('port-count').textContent==='0 / 1');
  assert(elements.get('limits').innerHTML.includes('本机固件未暴露可读取的 UCSI 设备节点或 WMI 数据面'));
  assert(elements.get('events').innerHTML.includes('本机未提供可读取的物理 USB-C 口插拔状态'));
  const noPorts={...state,snapshot:{...emptyState.snapshot,ports:[]},events:[]};
  window.CableRender(noPorts);
  assert(elements.get('port-grid').innerHTML.includes('本机未提供可读取的 UCSI 线缆身份数据'));
  window.CableRender(state);
}
elements.get('chart-mode').value='ports';elements.get('chart-mode').onchange();
assert(elements.get('chart-legend').innerHTML.length>0);
if(process.argv.includes('--tauri')){
  assert(window.CableLink.local&&window.CableLink.connected,'Tauri window must be treated as local');
  (async()=>{
    const r=await window.CableLink.request('api/state');
    assert(r.ok&&r.status===200,'IPC state response must be ok');
    assert((await r.json()).snapshot.platform==='Windows','IPC state payload must reach the UI');
    assert((await r.blob()) instanceof Blob,'IPC blob() must be usable for exports');
    const report=await window.CableLink.request('api/report');
    assert((await report.text()).includes('CableScope'),'IPC report text must be readable');
    const csv=await window.CableLink.request('api/history.csv');
    assert((await csv.blob()).size>0,'IPC CSV blob must be non-empty');
    const save=await window.CableLink.request('api/save',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({name:'我的线',notes:'',cable_info:{}})});
    assert(save.ok&&(await save.json()).ok===true,'IPC save response must be ok');
    assert(invocations.length>=5&&invocations.every(x=>x.name==='api_request'),'all IPC calls must use api_request');
    assert(invocations.find(x=>x.args.path==='api/save').args.options.body.name==='我的线','IPC save must forward a parsed JSON body');
    console.log('Tauri IPC transport, compatible Response and render passed.');
  })().catch(error=>{console.error(error);process.exitCode=1;});
}
console.log('UI rendering, power cards, diagrams, comparison, history filter and escaping passed. No browser layout claim.');
