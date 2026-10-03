const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

const source = fs.readFileSync(path.join(__dirname, 'collaboration.js'), 'utf8');
const context = {globalThis:{lificSession:{state:{user:{id:3}}}}, document:undefined, window:undefined, CustomEvent:class {}};
vm.runInNewContext(source, context);
const ui = context.globalThis.LificTopcoatIssueCollaboration;

test('comment rows escape server content and only offer author controls when enabled', () => {
  const comment = {id:7,user_id:3,author:'<script>',author_display_name:'<Admin>',created_at:'invalid',content:'<img src=x onerror=alert(1)>\n@sam'};
  const owner = ui.commentMarkup(comment, true);
  assert.match(owner, /&lt;Admin&gt;/);
  assert.match(owner, /&lt;img src=x onerror=alert\(1\)&gt;<br>@sam/);
  assert.match(owner, /data-comment-edit="7"/);
  assert.doesNotMatch(ui.commentMarkup(comment, false), /data-comment-delete/);
});

test('relation labels preserve each direction and render an empty state', () => {
  const list = {innerHTML:'',get children(){return {length:(this.innerHTML.match(/<li/g)||[]).length};}};
  const root = {dataset:{identifier:'ENG-2',editEnabled:'true',blockedBy:'ENG-1',blocks:'ENG-3',relatesTo:'',duplicates:'ENG-4',duplicatedBy:''},querySelector:()=>list};
  ui.renderRelations(root);
  assert.match(list.innerHTML, /blocked by: ENG-1/);
  assert.match(list.innerHTML, /blocks: ENG-3/);
  assert.match(list.innerHTML, /duplicates: ENG-4/);
  assert.equal((list.innerHTML.match(/data-relation-remove/g)||[]).length,3);
  root.dataset.blockedBy=''; root.dataset.blocks=''; root.dataset.duplicates='';
  ui.renderRelations(root);
  assert.match(list.innerHTML,/No relations/);
});

test('wait rows distinguish user/date blockers and expose clear only with edit access', () => {
  const list = {innerHTML:'',get children(){return {length:(this.innerHTML.match(/<li/g)||[]).length};}};
  const root = {dataset:{editEnabled:'false'},querySelector:()=>list};
  ui.renderWaits(root,[{id:9,kind:'user',username:'sam',display_name:'Sam',state:'holding',note:'Review'},{id:10,kind:'date',earliest:'2026-10-04',latest:'2026-10-05',state:'due',note:''}]);
  assert.match(list.innerHTML,/Waiting for Sam/);
  assert.match(list.innerHTML,/2026-10-04 through 2026-10-05/);
  assert.doesNotMatch(list.innerHTML,/data-wait-clear/);
  root.dataset.editEnabled='true'; ui.renderWaits(root,[{id:9,kind:'user',username:'sam',state:'holding'}]);
  assert.match(list.innerHTML,/data-wait-clear="9"/);
});
