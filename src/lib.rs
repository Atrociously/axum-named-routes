#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! AxumNamedRouter
//! This is a router that wraps the [`Router`](axum::Router) from [`axum`]
//! and automatically adds an [`Extension<Routes>`](axum::extract::Extension) layer.
//!
//! Check out [`NamedRouter`] and [`Routes`] for more information on how this works

use std::{collections::HashMap, convert::Infallible, path::{PathBuf, Path}, sync::Arc, task::Poll};

use axum::{
    body::{BoxBody, HttpBody},
    extract::{
        connect_info::IntoMakeServiceWithConnectInfo, rejection::ExtensionRejection, FromRequestParts,
    },
    http::Request,
    response::{Response, IntoResponse},
    routing::{future::RouteFuture, IntoMakeService, Route, MethodRouter},
    Extension, handler::Handler,
};
use futures::{future::BoxFuture, FutureExt, TryFutureExt};
use tower_layer::Layer;
use tower_service::Service;

type ServiceResp = Response<BoxBody>;
type ServiceErr = Infallible;
type String = std::borrow::Cow<'static, str>;

/// A mapping of all route names to their paths.
/// This can be used in requests as it implements [`FromRequest`](axum::extract::FromRequest)
/// It is also based on an [`Arc`](std::sync::Arc) internally so it can be cloned across requests
/// efficiently.
#[derive(Clone, Debug)]
pub struct Routes(Arc<HashMap<String, PathBuf>>);

impl Routes {
    /// Returns the route for the given name
    /// # Panics
    /// Panics if the name does not exist in routes
    pub fn has(&self, name: &str) -> &PathBuf {
        match self.0.get(name) {
            Some(path) => path,
            None => panic!("called `Routes::has` for a route that does not exist"),
        }
    }

    /// Tries to get the route for the given name
    /// if the route does not exist returns `None`
    pub fn get(&self, name: &str) -> Option<&PathBuf> {
        self.0.get(name)
    }

    /// Tries to get the route for the given name and takes an error
    /// to return if it does not exist
    pub fn get_or<E>(&self, name: &str, err: E) -> Result<&PathBuf, E> {
        self.0.get(name).ok_or(err)
    }

    /// Tries to get the route for the given name and takes an `FnOnce`
    /// to create an error if it does not exist
    pub fn get_or_else<F, E>(&self, name: &str, f: F) -> Result<&PathBuf, E>
    where
        F: FnOnce() -> E,
    {
        self.0.get(name).ok_or_else(f)
    }

    /// Find name by path
    ///
    /// This is a linear seach of the values within the map
    pub fn find(&self, path: impl AsRef<Path>) -> Option<&str> {
        let path = path.as_ref();
        for (k, v) in self.0.iter() {
            if v == path {
                return Some(k.as_ref());
            }
        }
        None
    }
}

impl Routes {
    fn new(map: HashMap<String, PathBuf>) -> Self {
        Self(Arc::new(map))
    }
}

impl<S: Send + Sync> FromRequestParts<S> for Routes {
    type Rejection = ExtensionRejection;

    fn from_request_parts<'life0,'life1,'async_trait>(parts: &'life0 mut axum::http::request::Parts, state: &'life1 S) -> BoxFuture<'async_trait, Result<Self, Self::Rejection>>
    where 
        'life0:'async_trait,
        'life1:'async_trait,
        Self:'async_trait
    {
        Extension::<Self>::from_request_parts(parts, state)
            .map_ok(|ext| ext.0)
            .boxed()
    }
}

/// Wraps the axum [`Router`](axum::Router) with an implementation
/// that builds a mapping of route names to paths.
///
/// Adds an [`Routes`] to the inner router as an [`Extension`](axum::extract::Extension) layer
/// when either [`into_make_service`](NamedRouter::into_make_service) or [`into_make_service_with_connect_info`](NamedRouter::into_make_service_with_connect_info)
/// are called.
#[derive(Debug)]
pub struct NamedRouter<S = (), B = axum::body::Body> {
    inner: axum::Router<S, B>,
    routes: HashMap<String, PathBuf>,
    nest_sep: String,
}

impl<S, B> NamedRouter<S, B>
where
    S: Clone + Send + Sync + 'static,
    B: HttpBody + Send + 'static,
{
    /// Create a new NamedRouter with default values.
    /// The default name separator is `.`
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a new NamedRouter with a different name separator than the default
    pub fn with_separator<T: Into<String>>(sep: T) -> Self {
        Self {
            nest_sep: sep.into(),
            ..Default::default()
        }
    }

    /// Set the separator for the router to use when nesting
    pub fn set_separator<T: Into<String>>(mut self, sep: T) -> Self {
        self.nest_sep = sep.into();
        self
    }

    /// The same as [`Router::fallback`](axum::Router::fallback)
    #[inline]
    pub fn fallback<H, T>(mut self, handler: H) -> Self
    where
        H: Handler<T, S, B>,
        T: 'static,
    {
        self.inner = self.inner.fallback(handler);
        self
    }

    /// The same as [`Router::fallback_service`](axum::Router::fallback_service)
    #[inline]
    pub fn fallback_service<T>(mut self, service: T) -> Self
    where
        T: Service<Request<B>, Error = ServiceErr> + Clone + Send + 'static,
        T::Response: IntoResponse,
        T::Future: Send + 'static,
    {
        self.inner = self.inner.fallback_service(service);
        self
    }

    /// The same as [`Router::layer`](axum::Router::layer)
    #[inline]
    pub fn layer<L, NewReqBody>(self, layer: L) -> NamedRouter<S, NewReqBody>
    where
        L: Layer<Route<B>> + Clone + Send + 'static,
        L::Service: Service<Request<NewReqBody>> + Clone + Send + 'static,
        <L::Service as Service<Request<NewReqBody>>>::Response: IntoResponse + 'static,
        <L::Service as Service<Request<NewReqBody>>>::Error: Into<Infallible> + 'static,
        <L::Service as Service<Request<NewReqBody>>>::Future: Send + 'static,
        NewReqBody: HttpBody + 'static,
    {
        let inner = self.inner.layer(layer);
        NamedRouter {
            inner,
            routes: self.routes,
            nest_sep: self.nest_sep,
        }
    }

    /// The merges the inner axum [`Router`](axum::Router) and the route map on this router
    pub fn merge<R>(mut self, other: R) -> Self
    where
        R: Into<NamedRouter<S, B>>,
    {
        let other = other.into();
        self.inner = self.inner.merge(other.inner);
        self.routes.extend(other.routes);
        self
    }

    /// Nests the inner axum [`Router`](axum::Router).
    ///
    /// When nesting the router adds `name` as a prefix to all route names in `router`.
    /// The the name nesting process looks essentially like `name + separator + route_name`
    /// for example:
    /// ```
    /// use std::path::PathBuf;
    /// use axum::routing::get;
    /// use axum_named_routes::{NamedRouter, Routes};
    ///
    /// async fn index(routes: Routes) -> &'static str {
    ///     "Hello, World!"
    /// }
    ///
    /// let ui_router: NamedRouter<(), axum::body::Body> = NamedRouter::new()
    ///     .route("index", "/", get(index));
    /// let base: NamedRouter<(), _> = NamedRouter::new()
    ///     .nest("ui", "/", ui_router)
    ///     .with_state(());
    ///
    /// let routes = base.routes();
    /// assert!(routes.get("ui.index").is_some());
    /// assert_eq!(routes.get("ui.index").unwrap(), &PathBuf::from("/"));
    ///
    /// base.into_make_service();
    /// ```
    ///
    /// Also ensures all paths in `router` are joined to `path` uses
    /// [`Path::join`](std::path::Path::join) like `path.join(route_path)`
    pub fn nest<N, P, R>(mut self, name: N, path: P, router: R) -> Self
    where
        N: Into<String>,
        P: AsRef<str>,
        R: Into<NamedRouter<S, B>>,
    {
        let name = name.into();
        let router = router.into();
        self.inner = self.inner.nest(path.as_ref(), router.inner);
        let path = PathBuf::from(path.as_ref());

        let prefixed_routes = router.routes.into_iter().map(|(inner_name, inner_path)| {
            // This is correct because axum routers panic when trying to insert a path that does
            // not start with a "/" meaning inner_path is guaranteed to start with a "/" but that
            // also means if we don't remove it then the path.join will fail to properly join the
            // paths as it will think inner_path is an absolute path
            let inner_path = inner_path.strip_prefix("/").unwrap();
            (
                name.clone() + self.nest_sep.clone() + inner_name,
                path.join(inner_path),
            )
        });
        self.routes.extend(prefixed_routes);

        self
    }

    /// Add a service the the router with a name and a path
    /// the name can then later be used to get a reference to the path
    pub fn route<N, P>(mut self, name: N, path: P, method_router: MethodRouter<S, B>) -> Self
    where
        N: Into<String>,
        P: AsRef<str>,
    {
        self.inner = self.inner.route(path.as_ref(), method_router);
        self.routes
            .insert(name.into(), PathBuf::from(path.as_ref()));
        self
    }

    /// The same as [`Router::route_layer`](axum::Router::route_layer)
    #[inline]
    pub fn route_layer<L>(mut self, layer: L) -> Self
    where
        L: Layer<Route<B>> + Clone + Send + 'static,
        L::Service: Service<Request<B>> + Clone + Send + 'static,
        <L::Service as Service<Request<B>>>::Response: IntoResponse + 'static,
        <L::Service as Service<Request<B>>>::Error: Into<Infallible> + 'static,
        <L::Service as Service<Request<B>>>::Future: Send + 'static,
    {
        self.inner = self.inner.route_layer(layer);
        self
    }

    /// Add a service the the router with a name and a path
    /// the name can then later be used to get a reference to the path
    pub fn route_service<N, P, T>(mut self, name: N, path: P, service: T) -> Self
    where
        N: Into<String>,
        P: AsRef<str>,
        T: Service<Request<B>, Error = ServiceErr> + Clone + Send + 'static,
        T::Response: IntoResponse,
        T::Future: Send + 'static,
    {
        self.inner = self.inner.route_service(path.as_ref(), service);
        self.routes
            .insert(name.into(), PathBuf::from(path.as_ref()));
        self
    }

    /// The same as [`Router::with_state`](axum::Router::with_state)
    pub fn with_state<S2>(self, state: S) -> NamedRouter<S2, B> {
        let inner = self.inner.with_state(state);
        NamedRouter {
            inner,
            routes: self.routes,
            nest_sep: self.nest_sep,
        }
    }

    /// Get a reference to the routes mapping before turning it into a [`Routes`]
    pub fn routes(&self) -> &HashMap<String, PathBuf> {
        &self.routes
    }

    /// Convert into a [`Router`](axum::Router) after adding an [`Routes`] as an [`Extension`](axum::extract::Extension) layer
    pub fn into_router(self) -> axum::Router<S, B> {
        self.inner.layer(Extension(Routes::new(self.routes)))
    }
}

impl<B> NamedRouter<(), B>
where
    B: HttpBody + Send + 'static,
{
    /// Uses [`Router::into_make_service`](axum::Router::into_make_service) after
    /// adding an [`Extension<Routes>`](axum::extract::Extension) layer to the inner router
    #[cfg(feature = "tokio")]
    pub fn into_make_service(self) -> IntoMakeService<axum::Router<(), B>> {
        let inner = self.into_router();
        inner.into_make_service()
    }

    /// Uses [`Router::into_make_service_with_connect_info`](axum::Router::into_make_service_with_connect_info) after
    /// adding an [`Extension<Routes>`](axum::extract::Extension) layer to the inner router
    #[cfg(feature = "tokio")]
    pub fn into_make_service_with_connect_info<C>(
        self,
    ) -> IntoMakeServiceWithConnectInfo<axum::Router<(), B>, C> {
        let inner = self.into_router();
        inner.into_make_service_with_connect_info()
    }
}

impl<S, B> Clone for NamedRouter<S, B>
where
    S: Clone,
{
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            routes: self.routes.clone(),
            nest_sep: self.nest_sep.clone(),
        }
    }
}

impl<B> Service<Request<B>> for NamedRouter<(), B>
where
    B: HttpBody + Send + 'static,
{
    type Response = ServiceResp;
    type Error = ServiceErr;
    type Future = RouteFuture<B, ServiceErr>;

    #[inline]
    fn poll_ready(
        &mut self,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    #[inline]
    fn call(&mut self, req: Request<B>) -> Self::Future {
        self.inner.call(req)
    }
}

impl<S, B> Default for NamedRouter<S, B>
where
    S: Clone + Send + Sync + 'static,
    B: HttpBody + Send + 'static,
{
    fn default() -> Self {
        Self {
            inner: axum::Router::new(),
            routes: HashMap::default(),
            nest_sep: ".".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::path::PathBuf;

    use crate::{NamedRouter, Routes};
    use axum::{routing::get, body::Body};

    async fn dummy(_routes: Routes) {}

    #[test]
    fn nesting() {
        let a = NamedRouter::<(), Body>::new().route("route_a", "/a", get(dummy));
        let b = NamedRouter::new()
            .route("route_a", "/a", get(dummy))
            .route("route_b", "/b", get(dummy));
        let c = NamedRouter::new().route("route_c", "/c", get(dummy));

        let app = NamedRouter::new()
            .nest("a", "/", a)
            .nest("b", "/b", b.merge(c.clone()))
            .nest("c", "/c", c);
        let routes = app.routes();

        assert!(routes.get("a.route_a").unwrap() == &PathBuf::from("/a"));
        assert!(routes.get("b.route_a").unwrap() == &PathBuf::from("/b/a"));
        assert!(routes.get("b.route_b").unwrap() == &PathBuf::from("/b/b"));
        assert!(routes.get("b.route_c").unwrap() == &PathBuf::from("/b/c"));
        assert!(routes.get("c.route_c").unwrap() == &PathBuf::from("/c/c"));
    }

    #[test]
    #[should_panic]
    fn route_overlap() {
        let a = NamedRouter::<(), Body>::new().route("route_a", "/a", get(dummy));
        let b = NamedRouter::new().route("route_a", "/a", get(dummy));
        NamedRouter::new().nest("a", "/", a).nest("b", "/", b);
    }
}
