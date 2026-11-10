'use strict';
const {spawn}=require('node:child_process');
const fs=require('node:fs'),path=require('node:path'),os=require('node:os'),assert=require('node:assert/strict');
const root=path.resolve(__dirname,'..');
const directory=process.platform==='darwin'?(process.arch==='arm64'?'mac-arm64':'mac'):(process.arch==='arm64'?'win-arm64-unpacked':'win-unpacked');
const executable=process.argv[2]||path.join(root,'dist/desktop',directory,...(process.platform==='darwin'?['CableScope.app','Contents','MacOS','CableScope']:['CableScope.exe']));
const temp=fs.mkdtempSync(path.join(os.tmpdir(),'cablescope-client-check-')),output=path.join(temp,'result.json');
const child=spawn(executable,['--smoke-test','--smoke-output='+output],{stdio:['ignore','pipe','pipe'],windowsHide:true,shell:false});
let errors='';child.stderr.on('data',data=>{errors=(errors+data).slice(-3000);});child.stdout.resume();
const timer=setTimeout(()=>{child.kill();console.error('Packaged Electron startup timed out.');process.exitCode=1;},90000);
child.once('error',error=>{clearTimeout(timer);console.error(error.message);process.exitCode=1;});
child.once('exit',code=>{
  clearTimeout(timer);
  try{
    assert.equal(code,0,'Electron exited unsuccessfully: '+errors);
    const result=JSON.parse(fs.readFileSync(output,'utf8'));
    assert.equal(result.ok,true,result.error);assert.equal(result.arch,process.arch);
    assert(result.ui&&result.save&&result.report);
    console.log('Packaged Electron + collector + rendered UI + save/report passed:',JSON.stringify(result));
  }catch(error){console.error(error.message);process.exitCode=1;}
  finally{fs.rmSync(temp,{recursive:true,force:true});}
});
