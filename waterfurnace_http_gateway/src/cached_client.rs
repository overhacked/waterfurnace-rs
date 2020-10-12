use async_trait::async_trait;
use cached::proc_macro::cached;
use waterfurnace_symphony as wf;

#[async_trait]
pub(crate) trait CachedClient {
    async fn cached_gateway_read(&self, awl_id: &str) -> wf::Result<wf::ReadResponse>;
}

#[async_trait]
impl CachedClient for wf::Client {
    async fn cached_gateway_read(&self, awl_id: &str) -> wf::Result<wf::ReadResponse> {
        cached_gateway_read_impl(self, awl_id).await
    }
}

#[cached(
time = 10,
key = "String",
convert = "{ awl_id.to_string() }",
result = true
)]
async fn cached_gateway_read_impl(client: &wf::Client, awl_id: &str) -> wf::Result<wf::ReadResponse> {
    client.gateway_read(awl_id).await
}
