'use strict';
const {spawn}=require('node:child_process');
const fs=require('node:fs'),path=require('node:path'),os=require('node:os'),assert=require('node:assert/strict');
const root=path.resolve(__dirname,'..');
const supplied=process.argv[2];
const exe=supplied||path.join(root,'target/release','CableScope'+(process.platform==='win32'?'.exe':''));
if(!supplied&&!fs.existsSync(exe)){console.error('找不到构建产物 '+exe+'，请先运行 cargo build --release -p CableScope，或显式传入可执行文件路径。');process.exit(1);}
const env={...process.env};
delete env.CABLESCOPE_COLLECTOR;
const temp=fs.mkdtempSync(path.join(os.tmpdir(),'cablescope-tauri-check-')),output=path.join(temp,'result.json');
const child=spawn(exe,['--smoke-test','--smoke-output='+output,'--smoke-data-dir='+path.join(temp,'runtime')],{stdio:['ignore','pipe','pipe'],windowsHide:true,shell:false,env});
let errors='';child.stderr.on('data',data=>{errors=(errors+data).slice(-3000);});child.stdout.resume();
const timer=setTimeout(()=>{child.kill();console.error('Tauri app smoke timed out.');process.exitCode=1;},90000);
child.once('error',error=>{clearTimeout(timer);console.error(error.message);process.exitCode=1;});
child.once('exit',code=>{
  clearTimeout(timer);
  try{
    assert.equal(code,0,'Tauri exited unsuccessfully: '+errors);
    const result=JSON.parse(fs.readFileSync(output,'utf8'));
    assert.equal(result.ok,true,result.error);
    assert(['x86_64','x64'].includes(result.arch)||(process.arch==='arm64'&&['aarch64','arm64'].includes(result.arch)));
    assert.equal(result.shell,'tauri');
    assert.equal(result.mode,'ipc','smoke must exercise the native IPC path');
    assert(['macOS','Windows'].includes(result.platform));
    assert(result.ui&&result.save&&result.report&&result.navigation);
    assert(Number.isInteger(result.ports)&&result.ports>0,'real hardware must expose non-empty ports');
    assert(result.devices>0,'real hardware must expose non-empty devices');
    console.log('Tauri IPC + native Engine + rendered window + save/report passed:',JSON.stringify(result));
  }catch(error){console.error(error.message);process.exitCode=1;}
  finally{fs.rmSync(temp,{recursive:true,force:true,maxRetries:10,retryDelay:200});}
});
