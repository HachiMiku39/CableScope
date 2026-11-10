'use strict';
const {app,BrowserWindow,Menu,dialog,shell}=require('electron');
const path=require('node:path');
const fs=require('node:fs');
const os=require('node:os');
const {startCollector}=require('./collector.cjs');
const SITE='https://cablescope-lab.leisurelyhorse4.chatgpt.site/';
const RELEASES='https://github.com/HachiMiku39/CableScope/releases/latest';
const smoke=process.argv.includes('--smoke-test');
const outputArg=process.argv.find(x=>x.startsWith('--smoke-output='));
let collector,window,allowQuit=false,quitting=false,smokeData;
app.setName('CableScope');
if(smoke){
  smokeData=fs.mkdtempSync(path.join(os.tmpdir(),'cablescope-electron-'));
  app.setPath('userData',path.join(smokeData,'profile'));
}
function external(url){if(url===SITE||url===RELEASES)shell.openExternal(url);}
function finishSmoke(result,code){
  if(outputArg)fs.writeFileSync(outputArg.slice('--smoke-output='.length),JSON.stringify(result));
  process.exitCode=code;app.quit();
}
function launchCommand(){
  if(app.isPackaged)return [path.join(process.resourcesPath,'collector',process.platform==='win32'?'CableScope.exe':'CableScope'),[]];
  return [process.env.CABLESCOPE_PYTHON||(process.platform==='win32'?'python':'python3'),[path.join(__dirname,'..','run.py')]];
}
async function open(){
  const [command,args]=launchCommand();
  collector=startCollector(command,[...args,'--no-browser','--port','0',...(smoke?['--data-dir',path.join(smokeData,'data')]:[])],{onExit:code=>{
    if(!quitting){if(!smoke&&code!==0)dialog.showErrorBox('采集已停止','本机采集程序已结束，重新打开 CableScope 可继续使用。');app.quit();}
  }});
  const base=await collector.ready;
  window=new BrowserWindow({width:1360,height:920,minWidth:850,minHeight:640,show:!smoke,backgroundColor:'#f5f6fa',title:'CableScope · 线缆实验室',
    webPreferences:{nodeIntegration:false,contextIsolation:true,sandbox:true,webSecurity:true,allowRunningInsecureContent:false}});
  window.webContents.setWindowOpenHandler(({url})=>{external(url);return {action:'deny'};});
  const block=(event,url)=>{if(!url.startsWith(base))event.preventDefault();};
  window.webContents.on('will-navigate',block);window.webContents.on('will-redirect',block);
  window.webContents.on('will-attach-webview',event=>event.preventDefault());
  window.webContents.session.setPermissionRequestHandler((_wc,_permission,callback)=>callback(false));
  window.webContents.session.setPermissionCheckHandler(()=>false);
  window.webContents.session.on('will-download',(_event,item)=>{
    item.setSaveDialogOptions({title:'导出 CableScope 报告',defaultPath:path.join(app.getPath('downloads'),item.getFilename())});
  });
  Menu.setApplicationMenu(Menu.buildFromTemplate([
    ...(process.platform==='darwin'?[{label:'CableScope',submenu:[{role:'about'},{type:'separator'},{role:'quit',label:'退出 CableScope'}]}]:[]),
    {label:'文件',submenu:[{label:'打开网页版',click:()=>external(SITE)},{type:'separator'},{role:'quit',label:'退出'}]},
    {label:'编辑',submenu:[{role:'undo'},{role:'redo'},{type:'separator'},{role:'cut'},{role:'copy'},{role:'paste'},{role:'selectAll'}]},
    {label:'显示',submenu:[{role:'reload'},{role:'resetZoom'},{role:'zoomIn'},{role:'zoomOut'},{role:'togglefullscreen'}]},
    {label:'帮助',submenu:[{label:'下载与更新',click:()=>external(RELEASES)}]}
  ]));
  await window.loadURL(base);
  if(smoke){
    const checks=await window.webContents.executeJavaScript(`(async()=>{
      const deadline=Date.now()+40000;let state;
      while(Date.now()<deadline){state=await (await fetch('api/state')).json();if(!state.snapshot.loading)break;await new Promise(r=>setTimeout(r,500));}
      if(!['macOS','Windows'].includes(state.snapshot.platform))throw Error('Native collector did not produce a snapshot');
      const saved=await fetch('api/save',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({name:'Electron smoke',notes:'synthetic test record',cable_info:{length_m:1}})});
      if(!saved.ok)throw Error('Native save failed');
      const report=await (await fetch('api/report')).json();
      if(report.records[0].name!=='Electron smoke')throw Error('Report missing saved record');
      window.CableRender(state);
      if(!document.getElementById('resistance-reading')||!document.getElementById('connection-title'))throw Error('UI incomplete');
      return {platform:state.snapshot.platform,ui:true,save:true,report:true};
    })()`);
    if(window.webContents.getLastWebPreferences().nodeIntegration)throw Error('Unexpected renderer privileges');
    finishSmoke({ok:true,...checks,electron:process.versions.electron,arch:process.arch},0);
  }
}
if(!app.requestSingleInstanceLock()){app.quit();}else{
  app.on('second-instance',()=>{if(window){if(window.isMinimized())window.restore();window.focus();}});
  app.on('window-all-closed',()=>app.quit());
  app.on('before-quit',event=>{
    if(allowQuit)return;
    event.preventDefault();if(quitting)return;quitting=true;
    Promise.resolve(collector?.stop()).finally(()=>{allowQuit=true;if(smokeData)fs.rmSync(smokeData,{recursive:true,force:true});app.quit();});
  });
  app.whenReady().then(open).catch(error=>{
    if(smoke)finishSmoke({ok:false,error:error.message},1);
    else{dialog.showErrorBox('CableScope 无法启动',error.message);app.quit();}
  });
}
