-- Decode HTML entities left in names and titles by clients that escaped
-- tool arguments before Lific stored text verbatim. A module stored as
-- "Wallpaper &amp; Polish" never matched a lookup for "Wallpaper & Polish".
--
-- Decoding is one level deep: &amp; is replaced last, so "&amp;lt;" becomes
-- "&lt;", not "<". Only rows whose text actually changes are written.
-- A module, folder or label whose decoded name would collide (case-
-- insensitively, as lookups compare) with another in the same project keeps
-- its stored name rather than creating an ambiguous pair.

UPDATE modules
   SET name = replace(replace(replace(replace(replace(replace(name,
              '&lt;', '<'), '&gt;', '>'), '&quot;', '"'), '&#39;', ''''),
              '&#x27;', ''''), '&amp;', '&')
 WHERE instr(name, '&') > 0
   AND name <> replace(replace(replace(replace(replace(replace(name,
              '&lt;', '<'), '&gt;', '>'), '&quot;', '"'), '&#39;', ''''),
              '&#x27;', ''''), '&amp;', '&')
   AND NOT EXISTS (
       SELECT 1 FROM modules AS other
        WHERE other.project_id = modules.project_id
          AND other.id <> modules.id
          AND other.name = replace(replace(replace(replace(replace(replace(modules.name,
              '&lt;', '<'), '&gt;', '>'), '&quot;', '"'), '&#39;', ''''),
              '&#x27;', ''''), '&amp;', '&') COLLATE NOCASE
   );

UPDATE folders
   SET name = replace(replace(replace(replace(replace(replace(name,
              '&lt;', '<'), '&gt;', '>'), '&quot;', '"'), '&#39;', ''''),
              '&#x27;', ''''), '&amp;', '&')
 WHERE instr(name, '&') > 0
   AND name <> replace(replace(replace(replace(replace(replace(name,
              '&lt;', '<'), '&gt;', '>'), '&quot;', '"'), '&#39;', ''''),
              '&#x27;', ''''), '&amp;', '&')
   AND NOT EXISTS (
       SELECT 1 FROM folders AS other
        WHERE other.project_id = folders.project_id
          AND other.id <> folders.id
          AND other.name = replace(replace(replace(replace(replace(replace(folders.name,
              '&lt;', '<'), '&gt;', '>'), '&quot;', '"'), '&#39;', ''''),
              '&#x27;', ''''), '&amp;', '&') COLLATE NOCASE
   );

UPDATE OR IGNORE labels
   SET name = replace(replace(replace(replace(replace(replace(name,
              '&lt;', '<'), '&gt;', '>'), '&quot;', '"'), '&#39;', ''''),
              '&#x27;', ''''), '&amp;', '&')
 WHERE instr(name, '&') > 0
   AND name <> replace(replace(replace(replace(replace(replace(name,
              '&lt;', '<'), '&gt;', '>'), '&quot;', '"'), '&#39;', ''''),
              '&#x27;', ''''), '&amp;', '&')
   AND NOT EXISTS (
       SELECT 1 FROM labels AS other
        WHERE other.project_id = labels.project_id
          AND other.id <> labels.id
          AND other.name = replace(replace(replace(replace(replace(replace(labels.name,
              '&lt;', '<'), '&gt;', '>'), '&quot;', '"'), '&#39;', ''''),
              '&#x27;', ''''), '&amp;', '&') COLLATE NOCASE
   );

UPDATE issues
   SET title = replace(replace(replace(replace(replace(replace(title,
               '&lt;', '<'), '&gt;', '>'), '&quot;', '"'), '&#39;', ''''),
               '&#x27;', ''''), '&amp;', '&')
 WHERE instr(title, '&') > 0
   AND title <> replace(replace(replace(replace(replace(replace(title,
               '&lt;', '<'), '&gt;', '>'), '&quot;', '"'), '&#39;', ''''),
               '&#x27;', ''''), '&amp;', '&');

UPDATE pages
   SET title = replace(replace(replace(replace(replace(replace(title,
               '&lt;', '<'), '&gt;', '>'), '&quot;', '"'), '&#39;', ''''),
               '&#x27;', ''''), '&amp;', '&')
 WHERE instr(title, '&') > 0
   AND title <> replace(replace(replace(replace(replace(replace(title,
               '&lt;', '<'), '&gt;', '>'), '&quot;', '"'), '&#39;', ''''),
               '&#x27;', ''''), '&amp;', '&');
