//! Per-connection IO loop: drives the bidi control stream, accepts auxiliary
//! uni streams (input events), and bridges to the daemon via inbound/outbound
//! channels.

use inputsync_protocol::Message;
use quinn::{Connection, RecvStream, SendStream};
use tokio::sync::mpsc;

pub async fn run_peer_loop(
    conn: Connection,
    control_send: SendStream,
    control_recv: RecvStream,
    outbound_rx: mpsc::Receiver<Message>,
    inbound_tx: mpsc::Sender<Message>,
) {
    let write_task = tokio::spawn(write_loop(control_send, outbound_rx));
    let read_task = tokio::spawn(read_loop(control_recv, inbound_tx.clone()));
    let aux_task = tokio::spawn(accept_streams_loop(conn, inbound_tx));

    tokio::select! {
        _ = write_task => {},
        _ = read_task => {},
        _ = aux_task => {},
    }
}

async fn write_loop(mut send: SendStream, mut outbound: mpsc::Receiver<Message>) {
    while let Some(msg) = outbound.recv().await {
        if let Err(e) = crate::stream::write_message(&mut send, &msg).await {
            tracing::debug!(error = %e, "control write failed; closing");
            break;
        }
    }
    let _ = send.finish();
}

async fn read_loop(mut recv: RecvStream, inbound: mpsc::Sender<Message>) {
    loop {
        match crate::stream::read_message(&mut recv).await {
            Ok(msg) => {
                if inbound.send(msg).await.is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

async fn accept_streams_loop(conn: Connection, inbound: mpsc::Sender<Message>) {
    loop {
        tokio::select! {
            stream = conn.accept_uni() => {
                match stream {
                    Ok(mut recv) => {
                        let inbound = inbound.clone();
                        tokio::spawn(async move {
                            loop {
                                match crate::stream::read_message(&mut recv).await {
                                    Ok(msg) => {
                                        if inbound.send(msg).await.is_err() {
                                            break;
                                        }
                                    }
                                    Err(_) => break,
                                }
                            }
                        });
                    }
                    Err(_) => break,
                }
            }
            stream = conn.accept_bi() => {
                match stream {
                    Ok((_send, mut recv)) => {
                        // Auxiliary bidi: clipboard pulls, file transfers.
                        // Read messages and forward them to the daemon.
                        let inbound = inbound.clone();
                        tokio::spawn(async move {
                            loop {
                                match crate::stream::read_message(&mut recv).await {
                                    Ok(msg) => {
                                        if inbound.send(msg).await.is_err() {
                                            break;
                                        }
                                    }
                                    Err(_) => break,
                                }
                            }
                        });
                    }
                    Err(_) => break,
                }
            }
        }
    }
}
