<?php
// Corpus site A: Gutenberg + Yoast + ACF + custom type/taxonomy. Run with `wp eval-file`.
// All content is synthetic (written for FerriteCMS tests).

function corpus_png($name, $w, $h, $rgb) {
    $im = imagecreatetruecolor($w, $h);
    imagefill($im, 0, 0, imagecolorallocate($im, $rgb[0], $rgb[1], $rgb[2]));
    $path = "/tmp/$name.png";
    imagepng($im, $path);
    return $path;
}
function corpus_media($name, $w, $h, $rgb, $parent = 0, $alt = '') {
    require_once ABSPATH . 'wp-admin/includes/image.php';
    require_once ABSPATH . 'wp-admin/includes/file.php';
    require_once ABSPATH . 'wp-admin/includes/media.php';
    $tmp = corpus_png($name, $w, $h, $rgb);
    $id = media_handle_sideload(['name' => "$name.png", 'tmp_name' => $tmp], $parent, ucfirst($name));
    if (is_wp_error($id)) { WP_CLI::error($id); }
    if ($alt) update_post_meta($id, '_wp_attachment_image_alt', $alt);
    return $id;
}

update_option('blogdescription', 'A synthetic corpus for importer tests');
update_option('permalink_structure', '/%year%/%monthnum%/%postname%/');
update_option('category_base', 'topics');

// Users.
$editor = wp_insert_user(['user_login' => 'erin', 'user_email' => 'erin@example.test', 'display_name' => 'Erin Editor', 'role' => 'editor', 'user_pass' => wp_generate_password()]);
$author = wp_insert_user(['user_login' => 'arlo', 'user_email' => 'arlo@example.test', 'display_name' => 'Arlo Author', 'role' => 'author', 'user_pass' => wp_generate_password()]);
$contrib = wp_insert_user(['user_login' => 'cass', 'user_email' => 'cass@example.test', 'display_name' => 'Cass Contributor', 'role' => 'contributor', 'user_pass' => wp_generate_password()]);

// Terms.
$news = wp_insert_term('News', 'category')['term_id'];
$local = wp_insert_term('Local News', 'category', ['parent' => $news, 'slug' => 'local'])['term_id'];
$guides = wp_insert_term('Guides', 'category', ['description' => 'How-to articles'])['term_id'];
foreach (['rust', 'wasm', 'self-hosting'] as $t) wp_insert_term(ucfirst($t), 'post_tag', ['slug' => $t]);

// Media.
$hero = corpus_media('hero', 1600, 900, [200, 60, 40], 0, 'A red hero banner');
$sq1 = corpus_media('square-one', 800, 800, [40, 120, 200], 0, 'Blue square');
$sq2 = corpus_media('square-two', 800, 800, [40, 200, 120]);
$hero_url = wp_get_attachment_url($hero);
$hero_med = wp_get_attachment_image_src($hero, 'medium')[0];
$sq1_url = wp_get_attachment_url($sq1);
$sq2_url = wp_get_attachment_url($sq2);

// A Gutenberg post covering the core blocks.
$blocks = <<<HTML
<!-- wp:paragraph -->
<p>Intro with <strong>bold</strong>, <em>italic</em>, <code>code</code> and <a href="https://example.org/">a link</a>.</p>
<!-- /wp:paragraph -->

<!-- wp:heading -->
<h2 class="wp-block-heading">A level two heading</h2>
<!-- /wp:heading -->

<!-- wp:heading {"level":3} -->
<h3 class="wp-block-heading">A level three heading</h3>
<!-- /wp:heading -->

<!-- wp:list -->
<ul class="wp-block-list"><!-- wp:list-item -->
<li>First item</li>
<!-- /wp:list-item -->

<!-- wp:list-item -->
<li>Second item<!-- wp:list -->
<ul class="wp-block-list"><!-- wp:list-item -->
<li>Nested item</li>
<!-- /wp:list-item --></ul>
<!-- /wp:list --></li>
<!-- /wp:list-item --></ul>
<!-- /wp:list -->

<!-- wp:list {"ordered":true,"start":3} -->
<ol start="3" class="wp-block-list"><!-- wp:list-item -->
<li>Three</li>
<!-- /wp:list-item -->

<!-- wp:list-item -->
<li>Four</li>
<!-- /wp:list-item --></ol>
<!-- /wp:list -->

<!-- wp:quote -->
<blockquote class="wp-block-quote"><!-- wp:paragraph -->
<p>A quoted paragraph.</p>
<!-- /wp:paragraph --><cite>Someone</cite></blockquote>
<!-- /wp:quote -->

<!-- wp:image {"id":$hero,"sizeSlug":"medium","linkDestination":"none"} -->
<figure class="wp-block-image size-medium"><img src="$hero_med" alt="A red hero banner" class="wp-image-$hero"/><figcaption class="wp-element-caption">The hero, medium size</figcaption></figure>
<!-- /wp:image -->

<!-- wp:gallery {"linkTo":"none"} -->
<figure class="wp-block-gallery has-nested-images columns-default is-cropped"><!-- wp:image {"id":$sq1,"sizeSlug":"large","linkDestination":"none"} -->
<figure class="wp-block-image size-large"><img src="$sq1_url" alt="Blue square" class="wp-image-$sq1"/></figure>
<!-- /wp:image -->

<!-- wp:image {"id":$sq2,"sizeSlug":"large","linkDestination":"none"} -->
<figure class="wp-block-image size-large"><img src="$sq2_url" alt="" class="wp-image-$sq2"/></figure>
<!-- /wp:image --></figure>
<!-- /wp:gallery -->

<!-- wp:code -->
<pre class="wp-block-code"><code>fn main() {
    println!("&lt;hello&gt;");
}</code></pre>
<!-- /wp:code -->

<!-- wp:preformatted -->
<pre class="wp-block-preformatted">Preformatted   text
  keeps spaces</pre>
<!-- /wp:preformatted -->

<!-- wp:table -->
<figure class="wp-block-table"><table class="has-fixed-layout"><thead><tr><th>Name</th><th>Value</th></tr></thead><tbody><tr><td>a</td><td>1</td></tr><tr><td>b</td><td>2</td></tr></tbody></table></figure>
<!-- /wp:table -->

<!-- wp:separator -->
<hr class="wp-block-separator has-alpha-channel-opacity"/>
<!-- /wp:separator -->

<!-- wp:embed {"url":"https://www.youtube.com/watch?v=dQw4w9WgXcQ","type":"video","providerNameSlug":"youtube","responsive":true,"className":"wp-embed-aspect-16-9 wp-has-aspect-ratio"} -->
<figure class="wp-block-embed is-type-video is-provider-youtube wp-block-embed-youtube wp-embed-aspect-16-9 wp-has-aspect-ratio"><div class="wp-block-embed__wrapper">
https://www.youtube.com/watch?v=dQw4w9WgXcQ
</div></figure>
<!-- /wp:embed -->

<!-- wp:buttons -->
<div class="wp-block-buttons"><!-- wp:button -->
<div class="wp-block-button"><a class="wp-block-button__link wp-element-button" href="https://example.org/signup">Sign up</a></div>
<!-- /wp:button --></div>
<!-- /wp:buttons -->

<!-- wp:columns -->
<div class="wp-block-columns"><!-- wp:column -->
<div class="wp-block-column"><!-- wp:paragraph -->
<p>Left column.</p>
<!-- /wp:paragraph --></div>
<!-- /wp:column -->

<!-- wp:column -->
<div class="wp-block-column"><!-- wp:paragraph -->
<p>Right column.</p>
<!-- /wp:paragraph --></div>
<!-- /wp:column --></div>
<!-- /wp:columns -->

<!-- wp:html -->
<div class="custom-widget" onclick="alert(1)">Raw <b>HTML</b> block<script>alert(2)</script></div>
<!-- /wp:html -->

<!-- wp:shortcode -->
[contact-form-7 id="12" title="Contact"]
<!-- /wp:shortcode -->

<!-- wp:acme/fancy-callout {"tone":"warm"} -->
<div class="wp-block-acme-fancy-callout">A block from a plugin the importer does not know.</div>
<!-- /wp:acme/fancy-callout -->

<!-- wp:paragraph -->
<p>See the <a href="http://localhost:8099/about/">about page</a> and <a href="http://localhost:8099/?p=PLACEHOLDER">an old ?p= link</a>.</p>
<!-- /wp:paragraph -->
HTML;

$post_id = wp_insert_post([
    'post_title' => 'Every core block', 'post_name' => 'every-core-block', 'post_status' => 'publish',
    'post_author' => $author, 'post_date' => '2024-03-15 10:30:00', 'post_excerpt' => 'A post with every core block.',
    'post_content' => $blocks, 'post_category' => [$news, $local], 'tags_input' => ['rust', 'wasm'],
]);
set_post_thumbnail($post_id, $hero);
WPSEO_Meta::set_value('title', 'Every core block %%sep%% %%sitename%%', $post_id);
WPSEO_Meta::set_value('metadesc', 'Yoast description for the block post.', $post_id);
WPSEO_Meta::set_value('canonical', 'https://canonical.example.org/blocks/', $post_id);
WPSEO_Meta::set_value('meta-robots-noindex', '1', $post_id);
WPSEO_Meta::set_value('meta-robots-nofollow', '1', $post_id);
WPSEO_Meta::set_value('meta-robots-adv', 'noarchive,nosnippet', $post_id);
WPSEO_Meta::set_value('focuskw', 'core blocks', $post_id);
update_post_meta($post_id, 'some_unrelated_meta', 'kept as wp.*');

// Pages with hierarchy and menu_order.
$about = wp_insert_post(['post_type' => 'page', 'post_title' => 'About', 'post_name' => 'about', 'post_status' => 'publish', 'menu_order' => 1,
    'post_content' => "<!-- wp:paragraph -->\n<p>About us.</p>\n<!-- /wp:paragraph -->", 'post_date' => '2023-01-01 09:00:00']);
$team = wp_insert_post(['post_type' => 'page', 'post_title' => 'Team', 'post_name' => 'team', 'post_parent' => $about, 'post_status' => 'publish', 'menu_order' => 2,
    'post_content' => "<!-- wp:paragraph -->\n<p>The team.</p>\n<!-- /wp:paragraph -->"]);
wp_update_post(['ID' => $post_id, 'post_content' => str_replace('?p=PLACEHOLDER', "?page_id=$team", $blocks)]);

// Statuses.
wp_insert_post(['post_title' => 'A draft', 'post_status' => 'draft', 'post_author' => $contrib, 'post_content' => '<!-- wp:paragraph --><p>Draft body.</p><!-- /wp:paragraph -->']);
wp_insert_post(['post_title' => 'Pending review', 'post_status' => 'pending', 'post_author' => $contrib, 'post_content' => '<!-- wp:paragraph --><p>Pending.</p><!-- /wp:paragraph -->']);
wp_insert_post(['post_title' => 'Private thoughts', 'post_status' => 'private', 'post_author' => $editor, 'post_content' => '<!-- wp:paragraph --><p>Private.</p><!-- /wp:paragraph -->']);
wp_insert_post(['post_title' => 'From the future', 'post_status' => 'future', 'post_date' => '2030-06-01 12:00:00', 'post_author' => $editor, 'post_category' => [$guides],
    'post_content' => '<!-- wp:paragraph --><p>Scheduled.</p><!-- /wp:paragraph -->']);
$trashed = wp_insert_post(['post_title' => 'Gone soon', 'post_status' => 'publish', 'post_content' => '<!-- wp:paragraph --><p>Trash me.</p><!-- /wp:paragraph -->']);
wp_trash_post($trashed);
$sticky = wp_insert_post(['post_title' => 'Sticky note', 'post_name' => 'sticky-note', 'post_status' => 'publish', 'post_date' => '2024-01-02 08:00:00',
    'post_content' => '<!-- wp:paragraph --><p>Pinned.</p><!-- /wp:paragraph -->', 'post_category' => [$guides], 'tags_input' => ['self-hosting']]);
stick_post($sticky);

// Comments: approved, threaded reply, held, spam.
$c1 = wp_insert_comment(['comment_post_ID' => $post_id, 'comment_author' => 'Visitor One', 'comment_author_email' => 'v1@example.test', 'comment_author_url' => 'https://v1.example.test',
    'comment_content' => 'Great post!', 'comment_approved' => 1, 'comment_date' => '2024-03-16 11:00:00']);
wp_insert_comment(['comment_post_ID' => $post_id, 'comment_parent' => $c1, 'user_id' => $author, 'comment_author' => 'Arlo Author', 'comment_author_email' => 'arlo@example.test',
    'comment_content' => 'Thanks!', 'comment_approved' => 1, 'comment_date' => '2024-03-16 12:00:00']);
wp_insert_comment(['comment_post_ID' => $post_id, 'comment_author' => 'Held Back', 'comment_author_email' => 'held@example.test', 'comment_content' => 'Awaiting moderation.', 'comment_approved' => 0]);
wp_insert_comment(['comment_post_ID' => $post_id, 'comment_author' => 'Spammer', 'comment_author_email' => 'spam@example.test', 'comment_content' => 'Buy now', 'comment_approved' => 'spam']);

// Custom post type + taxonomy registered by a mu-plugin (see corpus-types.php), ACF fields.
acf_import_field_group([
    'key' => 'group_book', 'title' => 'Book details',
    'fields' => [
        ['key' => 'field_isbn', 'label' => 'ISBN', 'name' => 'isbn', 'type' => 'text'],
        ['key' => 'field_pages', 'label' => 'Pages', 'name' => 'pages', 'type' => 'number'],
        ['key' => 'field_in_print', 'label' => 'In print', 'name' => 'in_print', 'type' => 'true_false'],
        ['key' => 'field_released', 'label' => 'Released', 'name' => 'released', 'type' => 'date_picker', 'return_format' => 'Y-m-d'],
        ['key' => 'field_blurb', 'label' => 'Blurb', 'name' => 'blurb', 'type' => 'textarea'],
        ['key' => 'field_cover', 'label' => 'Cover', 'name' => 'cover', 'type' => 'image', 'return_format' => 'id'],
        ['key' => 'field_format', 'label' => 'Format', 'name' => 'format', 'type' => 'select', 'choices' => ['hardback' => 'Hardback', 'paperback' => 'Paperback']],
        ['key' => 'field_site', 'label' => 'Site', 'name' => 'site', 'type' => 'url'],
        ['key' => 'field_review', 'label' => 'Review', 'name' => 'review', 'type' => 'wysiwyg'],
        ['key' => 'field_related', 'label' => 'Related', 'name' => 'related', 'type' => 'relationship', 'post_type' => ['book']],
        ['key' => 'field_quotes', 'label' => 'Quotes', 'name' => 'quotes', 'type' => 'repeater', 'sub_fields' => [
            ['key' => 'field_quote_text', 'label' => 'Text', 'name' => 'text', 'type' => 'text']]],
    ],
    'location' => [[['param' => 'post_type', 'operator' => '==', 'value' => 'book']]],
]);
$fantasy = wp_insert_term('Fantasy', 'genre')['term_id'];
wp_insert_term('High Fantasy', 'genre', ['parent' => $fantasy]);
$cover = corpus_media('cover', 600, 900, [90, 40, 160], 0, 'A purple cover');
$book1 = wp_insert_post(['post_type' => 'book', 'post_title' => 'The First Book', 'post_name' => 'the-first-book', 'post_status' => 'publish',
    'post_content' => "<!-- wp:paragraph -->\n<p>A book.</p>\n<!-- /wp:paragraph -->", 'post_date' => '2022-05-05 05:05:05']);
$book2 = wp_insert_post(['post_type' => 'book', 'post_title' => 'The Second Book', 'post_status' => 'publish',
    'post_content' => "<!-- wp:paragraph -->\n<p>Another book.</p>\n<!-- /wp:paragraph -->"]);
wp_set_object_terms($book1, [$fantasy], 'genre');
update_field('isbn', '978-3-16-148410-0', $book1);
update_field('pages', 412, $book1);
update_field('in_print', 1, $book1);
update_field('released', '20220505', $book1);
update_field('blurb', "Line one\nLine two", $book1);
update_field('cover', $cover, $book1);
update_field('format', 'hardback', $book1);
update_field('site', 'https://books.example.org/first', $book1);
update_field('review', '<p>A <em>fine</em> read.</p>', $book1);
update_field('related', [$book2], $book1);
update_field('quotes', [['text' => 'Quote one'], ['text' => 'Quote two']], $book1);

// Menus.
$menu = wp_create_nav_menu('Main');
$m_home = wp_update_nav_menu_item($menu, 0, ['menu-item-title' => 'Home', 'menu-item-url' => home_url('/'), 'menu-item-status' => 'publish', 'menu-item-type' => 'custom']);
$m_about = wp_update_nav_menu_item($menu, 0, ['menu-item-object-id' => $about, 'menu-item-object' => 'page', 'menu-item-type' => 'post_type', 'menu-item-status' => 'publish']);
wp_update_nav_menu_item($menu, 0, ['menu-item-object-id' => $team, 'menu-item-object' => 'page', 'menu-item-type' => 'post_type', 'menu-item-parent-id' => $m_about, 'menu-item-status' => 'publish']);
wp_update_nav_menu_item($menu, 0, ['menu-item-object-id' => $news, 'menu-item-object' => 'category', 'menu-item-type' => 'taxonomy', 'menu-item-status' => 'publish', 'menu-item-title' => 'All news']);
wp_update_nav_menu_item($menu, 0, ['menu-item-object-id' => $post_id, 'menu-item-object' => 'post', 'menu-item-type' => 'post_type', 'menu-item-status' => 'publish', 'menu-item-target' => '_blank']);
set_theme_mod('nav_menu_locations', ['primary' => $menu]);

WP_CLI::success("site A built: post $post_id");
