const {test} = require('node:test');
const assert = require('node:assert/strict');
const {STATUSES, PRIORITIES, labelNames} = require('./fields.js');
const fs = require('node:fs');
const vm = require('node:vm');

test('field choices preserve the issue API vocabulary and order', () => {
  assert.deepEqual(STATUSES, ['backlog','todo','active','done','cancelled']);
  assert.deepEqual(PRIORITIES, ['urgent','high','medium','low','none']);
});

test('labels preserve exact names instead of interpreting commas as delimiters', () => {
  assert.deepEqual(labelNames(['API, clients','urgent','API, clients']), ['API, clients','urgent']);
  assert.deepEqual(labelNames([' bug ','', 'docs', 'bug']), [' bug ','docs','bug']);
  assert.deepEqual(labelNames(null), []);
});

function fixture() {
  const element = () => Object.assign(new EventTarget(), {dataset:{},value:'',textContent:'',style:{setProperty(){}},
    append(){},replaceChildren(){},querySelectorAll:()=>[],contains:()=>false});
  const inputs = ['title','status','priority','module_id','target_date'].map(field=>Object.assign(element(),{dataset:{field}}));
  const name = element(), color = element(), create = element(), status = element();
  color.value='#123456';
  const nodes = {'[data-label-field]':element(),'[data-label-options]':element(),'[data-new-label-name]':name,
    '[data-new-label-color]':color,'[data-create-label]':create,'[data-fields-status]':status};
  const root = {querySelectorAll:()=>inputs,querySelector:selector=>nodes[selector] || null};
  const context = {document:{activeElement:null,createElement:element,createTextNode:text=>text}};
  inputs[0].blur=()=>{context.document.activeElement=null;inputs[0].dispatchEvent(new Event('blur'));};
  vm.runInNewContext(fs.readFileSync(require.resolve('./fields.js'),'utf8'),context);
  return {root,name,create,status,title:inputs[0],document:context.document,mount:context.LificTopcoatIssueFields.mount};
}
const settle = () => new Promise(resolve=>setImmediate(resolve));

test('blank titles on blur or Enter restore the stored title without submitting a mutation',async()=>{
  const {root,title,mount}=fixture();
  const writes=[];
  const current=mount(root,{issue:{title:'Stored title'},capabilities:{edit:true},onIntent:action=>{writes.push(action);}});
  title.value='  ';title.blur();await settle();
  assert.equal(writes.length,0);assert.equal(title.value,'Stored title');
  title.value='';const enter=new Event('keydown');Object.defineProperty(enter,'key',{value:'Enter'});title.dispatchEvent(enter);await settle();
  assert.equal(writes.length,0);assert.equal(title.value,'Stored title');
  current.dispose();
});

test('issue updates preserve a focused title draft and use the new baseline after cancellation',()=>{
  const {root,title,document,mount}=fixture();
  const current=mount(root,{issue:{title:'Stored title',seq:1},capabilities:{edit:true}});
  document.activeElement=title;title.value='Uncommitted title';
  current.update({title:'Latest stored title',seq:2});
  assert.equal(title.value,'Uncommitted title');
  const escape=new Event('keydown');Object.defineProperty(escape,'key',{value:'Escape'});title.dispatchEvent(escape);
  assert.equal(title.value,'Latest stored title');
  current.dispose();
});

test('remounting fields removes the disposed label creation handler', async () => {
  const {root,name,create,mount} = fixture();
  let creations=0;
  const props={issue:{labels:[]},capabilities:{edit:true},onCreateLabel:async label=>{creations++;return {name:label};},
    onIntent:async()=>({status:'applied',issue:{labels:['new-label']}})};
  mount(root,props).dispose();
  const current=mount(root,props);
  name.value='new-label'; create.dispatchEvent(new Event('click'));
  await settle();
  assert.equal(creations,1);
  current.dispose();
});

test('a disposed label creation response preserves the replacement fields', async () => {
  const {root,name,create,status,mount}=fixture();
  let complete, writes=0;
  const old=mount(root,{issue:{labels:[]},capabilities:{edit:true},
    onCreateLabel:()=>new Promise(resolve=>{complete=resolve;}),onIntent:()=>{writes++;}});
  name.value='old-label'; create.dispatchEvent(new Event('click')); old.dispose();
  const current=mount(root,{issue:{labels:[]},capabilities:{edit:true}});
  name.value='new account draft'; status.textContent='New account status';
  complete({name:'old-label'}); await settle();
  assert.equal(name.value,'new account draft');
  assert.equal(status.textContent,'New account status');
  assert.equal(writes,0);
  current.dispose();
});
