'use strict';
const {spawn}=require('node:child_process');
const fs=require('node:fs'),path=require('node:path'),os=require('node:os'),assert=require('node:assert/strict');
const root=path.resolve(__dirname,'..');
const supplied=process.argv[2];
const executable=supplied||path.join(root,'src-tauri/target/release','CableScope'+(process.platform==='win32'?'.exe':''));
const temp=fs.mkdtempSync(path.join(os.tmpdir(),'cablescope-tauri-check-')),output=path.join(temp,'result.json');
const child=spawn(executable,['--smoke-test','--smoke-output='+output,'--smoke-data-dir='+path.join(temp,'runtime')],{stdio:['ignore','pipe','pipe'],windowsHide:true,shell:false});
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
    assert(['macOS','Windows'].includes(result.platform));
    assert(result.ui&&result.save&&result.report&&result.navigation);
    console.log('Tauri + collector + rendered window + save/report passed:',JSON.stringify(result));
  }catch(error){console.error(error.message);process.exitCode=1;}
  finally{fs.rmSync(temp,{recursive:true,force:true,maxRetries:10,retryDelay:200});}
});
