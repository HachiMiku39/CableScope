'use strict';
const {test}=require('node:test'),assert=require('node:assert/strict');
const {localURL,startCollector}=require('../collector.cjs');
test('accept only a tokenized loopback URL from the owned collector',()=>{
  assert(localURL('http://127.0.0.1:47831/abcdefghijklmnopqrstuvwxyz/'));
  for(const url of ['http://127.0.0.1:0/abcdefghijklmnopqrstuvwxyz/','https://evil.invalid/abcdefghijklmnopqrstuvwxyz/','http://localhost:47831/abcdefghijklmnopqrstuvwxyz/','http://127.0.0.1:47831/short/','http://127.0.0.1:65536/abcdefghijklmnopqrstuvwxyz/'])assert.equal(localURL(url),null);
});
test('stop gracefully requests quit from the child that supplied the URL',async()=>{
  const code=`const http=require('node:http');const s=http.createServer((q,r)=>{r.end('{}');if(q.method==='POST'&&q.url==='/abcdefghijklmnopqrstuvwxyz/api/quit'){s.close(()=>process.exit(0));}});s.listen(0,'127.0.0.1',()=>console.log('http://127.0.0.1:'+s.address().port+'/abcdefghijklmnopqrstuvwxyz/'));`;
  const collector=startCollector(process.execPath,['-e',code]);
  assert(await collector.ready);await collector.stop();assert.equal(collector.child.exitCode,0);
});
test('child startup failure rejects without exposing its log',async()=>{
  const collector=startCollector(process.execPath,['-e',"console.error('private device name');process.exit(2)"]);
  await assert.rejects(collector.ready,/无法启动/);await collector.stop();
});
test('a process without the expected startup message is timed out',async()=>{
  const collector=startCollector(process.execPath,['-e','setInterval(()=>{},1000)'],{timeout:100});
  await assert.rejects(collector.ready,/超时/);await collector.stop();
});
