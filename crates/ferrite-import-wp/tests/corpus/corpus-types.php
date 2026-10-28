<?php
add_action('init', function () {
    register_post_type('book', ['label' => 'Books', 'public' => true, 'has_archive' => true, 'rewrite' => ['slug' => 'books'], 'show_in_rest' => true, 'supports' => ['title', 'editor', 'thumbnail', 'custom-fields']]);
    register_taxonomy('genre', ['book'], ['label' => 'Genres', 'hierarchical' => true, 'public' => true, 'rewrite' => ['slug' => 'genre'], 'show_in_rest' => true]);
});
