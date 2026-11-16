#[cfg(test)]
mod tests {
    use boring::ssl::{Ssl, SslContextBuilder, SslMethod};
    use cronet_rs::{Engine, EngineParamsBuilder};

    unsafe extern "C" {
        fn SSL_new(context: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
        fn cronet_bssl_SSL_new(context: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
    }

    #[test]
    fn independent_tls_implementations_link_and_initialize()
    -> Result<(), Box<dyn std::error::Error>> {
        assert_ne!(SSL_new as *const (), cronet_bssl_SSL_new as *const ());
        let context = SslContextBuilder::new(SslMethod::tls())?.build();
        let _ssl = Ssl::new(&context)?;
        let mut engine = Engine::new();
        engine.start_with_params(&EngineParamsBuilder::new().build()?)?;
        assert!(!engine.version().is_empty());
        engine.shutdown()?;
        Ok(())
    }
}
