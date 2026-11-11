// Exercise UI rendering as pure JavaScript, without browser or hardware access.
const fs=require('node:fs'),path=require('node:path'),vm=require('node:vm'),assert=require('node:assert/strict');
const root=path.resolve(__dirname,'..'),html=fs.readFileSync(path.join(root,'cablescope/web/index.html'),'utf8');
const ctx=new Proxy({},{get:()=>()=>{}}),elements=new Map();
for(const match of html.matchAll(/id="([^"]+)"/g))elements.set(match[1],{value:'',innerHTML:'',textContent:'',open:false,hidden:false,disabled:false,
  getBoundingClientRect:()=>({width:700,height:195}),getContext:()=>ctx,showModal(){this.open=true;},close(){this.open=false;}});
elements.get('chart-mode').value='system';
const window={devicePixelRatio:1,addEventListener(){},dispatchEvent(){}},document={getElementById:id=>{assert(elements.has(id),'Unknown UI id: '+id);return elements.get(id);},createElement:()=>({click(){}})};
const sandbox={window,document,location:{hostname:'127.0.0.1',pathname:'/AAAAAAAAAAAAAAAAAAAAAAAA/'},Headers,AbortSignal,Blob,URL,
  setTimeout:()=>0,setInterval:()=>0,Event:class{},fetch:()=>{throw Error('Network access is not part of this render test');},console};
vm.createContext(sandbox);
for(const file of ['connection.js','app.js'])vm.runInContext(fs.readFileSync(path.join(root,'cablescope/web',file),'utf8'),sandbox,{filename:file});
const port={id:'p1',name:'<img src=x onerror=bad>',kind:'USB-C',connected:true,orientation:'方向 1',transports:['USB3'],
  pin_configuration:{tx1:1},pin_map:{rows:[[{id:'A1',signal:'GND',class:'fixed'}]],note:'逻辑路由'},
  output_power:{watts:5.2,voltage_v:5.2,current_a:1},contract:{watts:100,voltage_v:20,current_a:5},
  negotiation:{notes:['测试'],source_reported_w:140},cable:{speed:'USB · 10 Gbps',current_a:5,vdo_hex:[],vendor_id:4660,product_id:1}};
const snapshot={at:'2026-11-10T09:00:00Z',platform:'macOS',power:{input_w:25,battery_net_w:0},ports:[port],devices:[],fabric:[],
  display_links:[{name:'DP',active:true,lane_count:4,link_rate_description:'HBR3'}],
  displays:[{name:'Monitor',width:1920,height:1080,refresh_hz:60,highest_mode:{width:3840,height:2160,refresh_hz:60},verdict:'可选更高模式'}],
  diagnostics:[],errors:[],limitations:[],resistance:{status:'estimate',milliohms:99.2,sample_count:40,r_squared:.99}};
const state=process.argv[2]?JSON.parse(fs.readFileSync(process.argv[2],'utf8')):{snapshot,
  records:[{name:'我的线',at:snapshot.at,notes:'人工记录',snapshot:{...snapshot,cable_info:{port_id:'p1',length_m:1,price_cny:39,softness:4}}}],
  events:[],history:[{at:snapshot.at,input_w:25,battery_net_w:0,port_power:[{id:'p1',name:'Port 1',output_w:5.2}]}]};
window.CableRender(state);
assert(elements.get('port-grid').innerHTML.includes('SMC 遥测'));
assert(elements.get('port-grid').innerHTML.includes('PD 合同'));
assert(elements.get('resistance-reading').innerHTML.includes('有效样本'));
if(!process.argv[2]){
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
console.log('UI rendering, power cards, diagrams, comparison, history filter and escaping passed. No browser layout claim.');
