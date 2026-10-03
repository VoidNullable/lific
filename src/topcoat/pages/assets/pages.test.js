const {test}=require('node:test');
const assert=require('node:assert/strict');
const fs=require('node:fs');
const vm=require('node:vm');

const source=fs.readFileSync(`${__dirname}/pages.js`,'utf8');
const scope={globalThis:{},URLSearchParams,Set,Map,Blob,FormData,Option:class{}};
vm.runInNewContext(source,scope);
const {markdown}=scope.globalThis.LificTopcoatPages;

test('markdown escapes raw HTML and keeps links, mentions and fenced code inert',()=>{
 const html=markdown('# Page\n\n<script>alert(1)</script> [docs](https://example.test/a) @riley and `@hidden`\n\n```html\n<img src=x onerror=alert(1)>\n@hidden\n```');
 assert.match(html,/<h1>Page<\/h1>/);assert.doesNotMatch(html,/<script>/);assert.doesNotMatch(html,/<img/);
 assert.match(html,/rel="nofollow noopener"/);assert.match(html,/<span class="tc-page-mention" data-page-mention="riley">@riley<\/span>/);
 assert.doesNotMatch(html,/data-page-mention="hidden"/);
 assert.match(html,/&lt;img src=x onerror=alert\(1\)&gt;/);
});

test('markdown retains tables, task lists, nested list indentation and code spans',()=>{
 const html=markdown('| Name | State |\n| --- | --- |\n| Page | active |\n\n- [x] Published\n  - `reader` access');
 assert.match(html,/<table>/);assert.match(html,/<th>Name<\/th>/);assert.match(html,/<td>Page<\/td>/);
 assert.match(html,/<input type="checkbox" disabled checked/);assert.match(html,/<ul>[\s\S]*<li><code>reader<\/code> access<\/li>/);
});

test('markdown preserves blank lines in fenced code and mixed paragraphs/lists',()=>{
 const html=markdown('```rust\nfn main() {\n\n    println!("<safe>");\n}\n```\n\nBefore list\n- first\n1. second\nAfter list');
 assert.match(html,/<pre><code>fn main\(\) \{\n\n    println!\(&quot;&lt;safe&gt;&quot;\);\n\}<\/code><\/pre>/);
 assert.match(html,/<p>Before list<\/p><ul><li>first<\/li><\/ul><ol><li>second<\/li><\/ol><p>After list<\/p>/);
});

test('markdown links do not double escape query separators',()=>{
 const html=markdown('[Search](https://example.test/?a=1&b=2)');
 assert.match(html,/href="https:\/\/example\.test\/\?a=1&amp;b=2"/);
 assert.doesNotMatch(html,/amp;amp/);
});
