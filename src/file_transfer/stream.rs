use eyre::eyre;
use tokio::io::AsyncRead;

#[derive(Debug, Clone, PartialEq, minicbor::Encode, minicbor::Decode)]
pub(crate) struct StreamHeader {
    #[cbor(n(0))]
    length: u64,
}

#[derive(Debug, Clone, PartialEq, minicbor::Encode, minicbor::Decode)]
pub(crate) struct StreamFooter {
    #[cbor(n(0))]
    status: u32,
}

pub(crate) async fn read_length<R>(reader: &mut R) -> eyre::Result<u64>
where
    R: futures::AsyncRead + Unpin,
{
    let buffer = vec![0u8; 16];
    let mut header_reader = minicbor_io::AsyncReader::with_buffer(reader, buffer);

    let header: Option<StreamHeader> = header_reader.read().await?;

    header
        .map(|h| h.length)
        .ok_or(eyre!("no legth field found in upload stream"))
}

// TODO create test to serialize and deserialize with insta
