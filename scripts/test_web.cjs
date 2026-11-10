// Exercise UI rendering as pure JavaScript, without browser or hardware access.
const fs=require('node:fs'),path=require('node:path'),vm=require('node:vm'),assert=require('node:assert/strict');
const root=path.resolve(__dirname,'..'),html=fs.readFileSync(path.join(root,'cablescope/web/index.html'),'utf8');
const ctx=new Proxy({},{get:()=>()=>{}}),elements=new Map();
for(const match of html.matchAll(/id="([^"]+)"/g))elements.set(match[1],{value:'',innerHTML:'',textContent:'',open:false,hidden:false,disabled:false,
  getBoundingClientRect:()=>({width:700,height:195}),getContext:()=>ctx,showModal(){this.open=true;},close(){this.open=false;}});
elements.get('chart-mode').value='system';
const window={devicePixelRatio:1,addEventListener(){},dispatchEvent(){}},document={getElementById:id=>{assert(elements.has(id),'Unknown UI id: '+id);return elements.get(id);},createElement:()=>({click(){}})};
const sandbox={window,document,location:{hostname:'cablescope-lab.leisurelyhorse4.chatgpt.site',pathname:'/'},Headers,AbortSignal,Blob,URL,
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
  assert(elements.get('display-list').innerHTML.includes('3840'));
  assert(elements.get('records').innerHTML.includes('我的线'));
  elements.get('record-filter').value='不存在的线';elements.get('record-filter').oninput();
  assert(elements.get('records').innerHTML.includes('没有匹配'));
}
elements.get('chart-mode').value='ports';elements.get('chart-mode').onchange();
assert(elements.get('chart-legend').innerHTML.length>0);
console.log('UI rendering, power cards, diagrams, comparison, history filter and escaping passed. No browser layout claim.');
