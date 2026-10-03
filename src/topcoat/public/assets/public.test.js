const {test}=require('node:test');
const assert=require('node:assert/strict');
const publicUi=require('./public.js');

test('public byte formatting is stable for scrubbed attachment DTOs',()=>{
 assert.equal(publicUi.formatBytes(4),'4 B');
 assert.equal(publicUi.formatBytes(1536),'1.5 KB');
 assert.equal(publicUi.formatBytes(undefined),'');
});

test('renderer exports no DOM-dependent controller construction on the Node path',()=>{
 assert.equal(typeof publicUi.PublicController,'function');
 assert.equal(typeof publicUi.renderMarkdown,'function');
 assert.equal(typeof publicUi.attach,'function');
});
