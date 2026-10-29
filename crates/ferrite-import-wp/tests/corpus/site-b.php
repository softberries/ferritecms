<?php
// Corpus site B: classic editor HTML + shortcodes + Rank Math. Run with `wp eval-file` on an emptied site.
// All content is synthetic (written for FerriteCMS tests).
require_once ABSPATH . 'wp-admin/includes/image.php';
require_once ABSPATH . 'wp-admin/includes/file.php';
require_once ABSPATH . 'wp-admin/includes/media.php';

function corpus_media($name, $w, $h, $rgb, $parent = 0, $alt = '', $ext = 'jpg') {
    $im = imagecreatetruecolor($w, $h);
    imagefill($im, 0, 0, imagecolorallocate($im, $rgb[0], $rgb[1], $rgb[2]));
    $tmp = "/tmp/$name.$ext";
    $ext === 'jpg' ? imagejpeg($im, $tmp, 85) : imagepng($im, $tmp);
    $id = media_handle_sideload(['name' => "$name.$ext", 'tmp_name' => $tmp], $parent, ucfirst($name));
    if (is_wp_error($id)) { WP_CLI::error($id); }
    if ($alt) update_post_meta($id, '_wp_attachment_image_alt', $alt);
    return $id;
}

update_option('blogname', 'Classic Corpus');
update_option('blogdescription', 'Classic editor, shortcodes and Rank Math');
update_option('permalink_structure', '/%category%/%postname%/');
update_option('tag_base', 'labels');

$writer = wp_insert_user(['user_login' => 'wren', 'user_email' => 'wren@example.test', 'display_name' => 'Wren Writer', 'role' => 'author', 'user_pass' => wp_generate_password()]);
$travel = wp_insert_term('Travel', 'category')['term_id'];
$food = wp_insert_term('Food & Drink', 'category', ['slug' => 'food-drink'])['term_id'];
wp_insert_term('Europe', 'post_tag');

$photo = corpus_media('harbour', 1200, 800, [30, 90, 140], 0, 'A harbour at dusk');
$photo_300 = wp_get_attachment_image_src($photo, 'medium')[0];
$g1 = corpus_media('market', 1024, 768, [160, 120, 40]);
$g2 = corpus_media('bridge', 1024, 768, [60, 60, 60], 0, 'A stone bridge');
$doc = (function () {
    $tmp = '/tmp/menu.pdf';
    file_put_contents($tmp, "%PDF-1.4\n1 0 obj<</Type/Catalog>>endobj\ntrailer<</Root 1 0 R>>\n%%EOF\n");
    return media_handle_sideload(['name' => 'menu.pdf', 'tmp_name' => $tmp], 0, 'Lunch menu');
})();
$doc_url = wp_get_attachment_url($doc);

// Classic content: no block comments, double newlines as paragraphs (wpautop), shortcodes.
$classic = <<<HTML
First paragraph written in the classic editor.
It has a single line break here.

Second paragraph with <strong>bold</strong> and <a href="https://example.org/" target="_blank" rel="noopener">an external link</a>.

[caption id="attachment_$photo" align="aligncenter" width="300"]<img class="size-medium wp-image-$photo" src="$photo_300" alt="A harbour at dusk" width="300" height="200" /> The harbour, captioned[/caption]

<h2>A classic heading</h2>
<ul>
 	<li>Classic list one</li>
 	<li>Classic list two</li>
</ul>
<blockquote>A classic quote.</blockquote>
[gallery ids="$g1,$g2" columns="2"]

[embed]https://vimeo.com/76979871[/embed]

https://www.youtube.com/watch?v=aqz-KE-bpKQ

[video src="https://cdn.example.org/clip.mp4"]

Download the <a href="$doc_url">lunch menu (PDF)</a>.

[unknown_plugin_shortcode foo="bar"]Inner text[/unknown_plugin_shortcode]

<!--more-->

Text after the more tag.
<pre>preformatted
  classic</pre>
<table><tr><td>x</td><td>y</td></tr></table>
<p style="text-align: center;"><span style="color: #ff0000; text-decoration: underline;">Styled</span> text<sup>2</sup>.</p>
<h1>An h1 inside the body</h1>
<img src="http://localhost:8099/wp-content/uploads/not-in-library.jpg" alt="missing file" />
HTML;

$p1 = wp_insert_post(['post_title' => 'Harbour walk', 'post_name' => 'harbour-walk', 'post_status' => 'publish', 'post_author' => $writer,
    'post_date' => '2021-07-04 18:00:00', 'post_content' => $classic, 'post_category' => [$travel], 'tags_input' => ['Europe']]);
wp_update_post(['ID' => $photo, 'post_parent' => $p1]);
set_post_thumbnail($p1, $photo);
update_post_meta($p1, 'rank_math_title', '%title% %sep% %sitename%');
update_post_meta($p1, 'rank_math_description', 'Rank Math description for the harbour walk.');
update_post_meta($p1, 'rank_math_robots', ['noindex', 'nofollow']);
update_post_meta($p1, 'rank_math_advanced_robots', ['max-snippet' => '-1', 'max-video-preview' => '-1', 'max-image-preview' => 'large']);
update_post_meta($p1, 'rank_math_canonical_url', 'https://canonical.example.org/harbour/');
update_post_meta($p1, 'rank_math_focus_keyword', 'harbour,walk');
update_post_meta($p1, 'rank_math_primary_category', $travel);

$p2 = wp_insert_post(['post_title' => 'Café review — “quotes” & ampersands', 'post_status' => 'publish', 'post_author' => $writer, 'post_date' => '2021-08-01 08:00:00',
    'post_content' => "Short classic post linking back to <a href=\"" . get_permalink($p1) . "\">the harbour walk</a> and an image <a href=\"" . wp_get_attachment_url($g1) . "\"><img src=\"" . wp_get_attachment_image_src($g1, 'thumbnail')[0] . "\" alt=\"\" /></a>.",
    'post_category' => [$food]]);
update_post_meta($p2, 'rank_math_robots', ['index', 'follow']);

$page = wp_insert_post(['post_type' => 'page', 'post_title' => 'Contact', 'post_status' => 'publish', 'post_content' => "Write to us.\n\n[contact-form-7 id=\"5\"]"]);

$c = wp_insert_comment(['comment_post_ID' => $p1, 'comment_author' => 'Reader', 'comment_author_email' => 'reader@example.test',
    'comment_content' => "Lovely <a href=\"https://spam.example\">photos</a>!\n\nSecond paragraph <b>bold</b>.", 'comment_approved' => 1, 'comment_date' => '2021-07-05 09:00:00']);
wp_insert_comment(['comment_post_ID' => $p1, 'comment_type' => 'pingback', 'comment_author' => 'Some blog', 'comment_author_url' => 'https://blog.example/post',
    'comment_content' => '[…] linked here […]', 'comment_approved' => 1]);

WP_CLI::success("site B built: post $p1");
