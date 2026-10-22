//! Generated host bindings for `ferrite:plugin@0.1.0` and conversions to/from
//! the host-side types in `types.rs`.

pub(crate) mod v0_1 {
    wasmtime::component::bindgen!({
        world: "plugin",
        path: "../../wit/ferrite-plugin@0.1.0",
        imports: { default: async },
        exports: { default: async },
    });
}

use crate::types as host;
use v0_1::ferrite::plugin::types as wit;

impl From<host::TermRef> for wit::TermRef {
    fn from(v: host::TermRef) -> Self {
        Self {
            taxonomy: v.taxonomy,
            slug: v.slug,
            name: v.name,
        }
    }
}

impl From<wit::TermRef> for host::TermRef {
    fn from(v: wit::TermRef) -> Self {
        Self {
            taxonomy: v.taxonomy,
            slug: v.slug,
            name: v.name,
        }
    }
}

impl From<host::Post> for wit::Post {
    fn from(v: host::Post) -> Self {
        Self {
            id: v.id,
            kind: v.kind,
            slug: v.slug,
            title: v.title,
            excerpt: v.excerpt,
            status: v.status,
            published: v.published,
            url: v.url,
            author: v.author,
            terms: v.terms.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<wit::Post> for host::Post {
    fn from(v: wit::Post) -> Self {
        Self {
            id: v.id,
            kind: v.kind,
            slug: v.slug,
            title: v.title,
            excerpt: v.excerpt,
            status: v.status,
            published: v.published,
            url: v.url,
            author: v.author,
            terms: v.terms.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<host::MetaTag> for wit::MetaTag {
    fn from(v: host::MetaTag) -> Self {
        Self {
            name: v.name,
            property: v.property,
            content: v.content,
        }
    }
}

impl From<wit::MetaTag> for host::MetaTag {
    fn from(v: wit::MetaTag) -> Self {
        Self {
            name: v.name,
            property: v.property,
            content: v.content,
        }
    }
}

impl From<host::LinkTag> for wit::LinkTag {
    fn from(v: host::LinkTag) -> Self {
        Self {
            rel: v.rel,
            href: v.href,
            hreflang: v.hreflang,
            kind: v.kind,
        }
    }
}

impl From<wit::LinkTag> for host::LinkTag {
    fn from(v: wit::LinkTag) -> Self {
        Self {
            rel: v.rel,
            href: v.href,
            hreflang: v.hreflang,
            kind: v.kind,
        }
    }
}

impl From<host::ScriptTag> for wit::ScriptTag {
    fn from(v: host::ScriptTag) -> Self {
        Self {
            src: v.src,
            json_ld: v.json_ld,
        }
    }
}

impl From<wit::ScriptTag> for host::ScriptTag {
    fn from(v: wit::ScriptTag) -> Self {
        Self {
            src: v.src,
            json_ld: v.json_ld,
        }
    }
}

impl From<host::Head> for wit::Head {
    fn from(v: host::Head) -> Self {
        Self {
            title: v.title,
            metas: v.metas.into_iter().map(Into::into).collect(),
            links: v.links.into_iter().map(Into::into).collect(),
            scripts: v.scripts.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<wit::Head> for host::Head {
    fn from(v: wit::Head) -> Self {
        Self {
            title: v.title,
            metas: v.metas.into_iter().map(Into::into).collect(),
            links: v.links.into_iter().map(Into::into).collect(),
            scripts: v.scripts.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<host::FilterValue> for wit::FilterValue {
    fn from(v: host::FilterValue) -> Self {
        match v {
            host::FilterValue::Head(h) => Self::Head(h.into()),
            host::FilterValue::Html(s) => Self::Html(s),
            host::FilterValue::Text(s) => Self::Text(s),
        }
    }
}

impl From<wit::FilterValue> for host::FilterValue {
    fn from(v: wit::FilterValue) -> Self {
        match v {
            wit::FilterValue::Head(h) => Self::Head(h.into()),
            wit::FilterValue::Html(s) => Self::Html(s),
            wit::FilterValue::Text(s) => Self::Text(s),
        }
    }
}

impl From<host::RequestInfo> for wit::RequestInfo {
    fn from(v: host::RequestInfo) -> Self {
        Self {
            path: v.path,
            locale: v.locale,
            is_logged_in: v.is_logged_in,
            post: v.post,
        }
    }
}

impl From<wit::RequestInfo> for host::RequestInfo {
    fn from(v: wit::RequestInfo) -> Self {
        Self {
            path: v.path,
            locale: v.locale,
            is_logged_in: v.is_logged_in,
            post: v.post,
        }
    }
}

impl From<host::ActionEvent> for wit::ActionEvent {
    fn from(v: host::ActionEvent) -> Self {
        Self {
            subject: v.subject,
            payload: v.payload.to_string(),
        }
    }
}

impl TryFrom<wit::ActionEvent> for host::ActionEvent {
    type Error = serde_json::Error;

    fn try_from(v: wit::ActionEvent) -> Result<Self, Self::Error> {
        Ok(Self {
            subject: v.subject,
            payload: serde_json::from_str(&v.payload)?,
        })
    }
}

impl From<host::RouteRequest> for wit::RouteRequest {
    fn from(v: host::RouteRequest) -> Self {
        Self {
            method: v.method,
            path: v.path,
            query: v.query,
            headers: v.headers,
            body: v.body,
            client_ip_hash: v.client_ip_hash,
            user: v.user,
        }
    }
}

impl From<wit::RouteRequest> for host::RouteRequest {
    fn from(v: wit::RouteRequest) -> Self {
        Self {
            method: v.method,
            path: v.path,
            query: v.query,
            headers: v.headers,
            body: v.body,
            client_ip_hash: v.client_ip_hash,
            user: v.user,
        }
    }
}

impl From<host::RouteResponse> for wit::RouteResponse {
    fn from(v: host::RouteResponse) -> Self {
        Self {
            status: v.status,
            headers: v.headers,
            body: v.body,
        }
    }
}

impl From<wit::RouteResponse> for host::RouteResponse {
    fn from(v: wit::RouteResponse) -> Self {
        Self {
            status: v.status,
            headers: v.headers,
            body: v.body,
        }
    }
}
