const {test}=require('node:test');
const assert=require('node:assert/strict');
const files=require('./files.js');

test('files list keeps the legacy MIME, sort and page-size contract',()=>{
 assert.deepEqual(files.MIME_FILTERS,[null,'image','video','audio','text','pdf','archive','other']);
 assert.deepEqual(files.SORTS,['created_at','size','filename']);
 assert.equal(files.PAGE_SIZE,50);
});

test('project role and uploader gates match attachment deletion policy',()=>{
 assert.equal(files.canDelete({uploaderId:4,viewerId:4,isAdmin:false,canEdit:false}),true);
 assert.equal(files.canDelete({uploaderId:3,viewerId:4,isAdmin:false,canEdit:false}),false);
 assert.equal(files.canDelete({uploaderId:null,viewerId:4,isAdmin:false,canEdit:false}),false);
 assert.equal(files.canDelete({uploaderId:3,viewerId:4,isAdmin:false,canEdit:true}),true);
 assert.equal(files.canDelete({uploaderId:3,viewerId:4,isAdmin:true,canEdit:false}),true);
 assert.equal(files.canDelete({uploaderId:null,viewerId:null,isAdmin:false,canEdit:false}),true);
 assert.equal(files.canDelete({uploaderId:8,viewerId:4,isAdmin:false,canEdit:true,orphan:true}),false);
 assert.equal(files.canDelete({uploaderId:4,viewerId:4,isAdmin:false,canEdit:true,orphan:true}),true);
 assert.equal(files.canDelete({uploaderId:8,viewerId:4,isAdmin:true,canEdit:false,orphan:true}),true);
});

test('where-used links use the attachment API entity shape and keep project scope',()=>{
 assert.equal(files.entityHref('ENG',{entity_type:'issue',entity_id:42,identifier:'ENG-42',title:'Issue'}),'/ENG/issues/ENG-42');
 assert.equal(files.entityHref('ENG',{entity_type:'page',entity_id:42,identifier:'ENG-PG-42',title:'Page'}),'/ENG/pages/42');
 assert.equal(files.entityHref('ENG',{entity_type:'comment',entity_id:7,identifier:null,title:'Comment'}),null);
 assert.equal(files.entityHref('ENG',{entity_type:'issue',entity_id:42,identifier:'OTHER-42',title:'Other project issue'}),'/OTHER/issues/OTHER-42');
 assert.equal(files.entityHref('ENG',{entity_type:'page',entity_id:84,identifier:'OTHER-DOC-12',title:'Other project page'}),'/OTHER/pages/84');
});

test('file sizes and orphan countdowns stay readable at boundary values',()=>{
 assert.equal(files.formatBytes(0),'0 B');
 assert.equal(files.formatBytes(1536),'1.5 KB');
 assert.equal(files.formatCountdown(0),'swept on the next pass');
 assert.equal(files.formatCountdown(3601),'swept in 1h');
 assert.equal(files.formatCountdown(86400),'swept in 1 day');
 assert.equal(files.formatCountdown(20),'swept in 1 min');
});

test('CSV and TSV preserve quoted fields, BOM, ragged rows, numeric sorting, and the inline row cap',()=>{
 const csv=files.parseDelimited('\ufeffname,count,note\r\n"a,b",10,"first\nsecond"\r\n"say ""hello""",2,<script>danger</script>\r\n');
 assert.deepEqual(csv.headers,['name','count','note']);assert.equal(csv.rows[0][0],'a,b');assert.equal(csv.rows[0][2],'first\nsecond');assert.equal(csv.rows[1][0],'say "hello"');
 assert.deepEqual(files.sortRows(csv.rows,1,'asc').map(row=>row[1]),['2','10']);
 assert.equal(files.detectDelimiter('report.tsv','one\ttwo'),'\t');assert.equal(files.detectDelimiter('report.csv','one,two'),',');
 const large=files.parseDelimited('a,b\n'+Array.from({length:205},(_,i)=>`${i},value,extra`).join('\n'));
 assert.equal(large.rows.length,200);assert.equal(large.totalRows,205);assert.equal(large.columnCount,3);assert.equal(large.truncated,true);
 assert.deepEqual(files.parseDelimited('a\tb\n1\t2',{delimiter:'\t'}).rows,[['1','2']]);
 assert.deepEqual(files.sortRows([['10'],[''],['2']],0,'desc'),[['10'],['2'],['']]);
});

test('unified diff previews preserve file boundaries, line numbers, renames, binary markers, and totals',()=>{
 const diff=files.parseUnifiedDiff('diff --git a/old.txt b/new.txt\n--- a/old.txt\n+++ b/new.txt\n@@ -3,2 +4,2 @@\n unchanged\n-<script>old</script>\n+<img src=x onerror=bad()>\ndiff --git a/picture b/picture\nBinary files a/picture and b/picture differ\n');
 assert.equal(diff.files.length,2);assert.equal(diff.files[0].display,'old.txt -> new.txt');assert.equal(diff.files[1].binary,true);
 assert.deepEqual(diff.files[0].lines.filter(line=>['add','del','context'].includes(line.kind)).map(line=>[line.kind,line.oldNo,line.newNo]),[['context',3,4],['del',4,null],['add',null,5]]);
 assert.equal(files.summarizeDiff(diff),'2 files changed, +1 -1');
});
