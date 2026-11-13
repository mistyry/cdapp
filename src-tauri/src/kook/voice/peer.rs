use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use rtc::media::Sample;
use rtc::peer_connection::certificate::{CertificateParams, RTCCertificate};
use rtc::peer_connection::transport::RTCDtlsRole;
use rtc::rtp::extension::HeaderExtension;
use rtc::rtp::extension::audio_level_extension::AudioLevelExtension;
use rtc::rtp_transceiver::RTCRtpTransceiverDirection;
use rtc::rtp_transceiver::rtp_sender::{
    RTCRtpCodec, RTCRtpCodecParameters, RTCRtpCodingParameters, RTCRtpEncodingParameters,
    RTCRtpHeaderExtensionCapability, RtpCodecKind,
};
use rtc::shared::marshal::{Marshal, MarshalSize};
use tokio::sync::mpsc;
use webrtc::media_stream::MediaStreamTrack;
use webrtc::media_stream::track_local::TrackLocal;
use webrtc::media_stream::track_local::static_sample::TrackLocalStaticSample;
use webrtc::media_stream::track_remote::{TrackRemote, TrackRemoteEvent};
use webrtc::peer_connection::{
    MediaEngine, PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler,
    RTCConfigurationBuilder, RTCPeerConnectionState, RTCSessionDescription, Registry,
    SettingEngineBuilder, register_default_interceptors,
};

use super::sdp::{LEVEL_URI, LocalOffer, MID_URI, OPUS_PT};
use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Leg {
    Send,
    Recv,
}

#[derive(Debug)]
pub enum MediaEvent {
    State(Leg, PcState),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PcState {
    New,
    Connecting,
    Connected,
    Disconnected,
    Failed,
    Closed,
}

impl From<RTCPeerConnectionState> for PcState {
    fn from(s: RTCPeerConnectionState) -> Self {
        match s {
            RTCPeerConnectionState::Connecting => Self::Connecting,
            RTCPeerConnectionState::Connected => Self::Connected,
            RTCPeerConnectionState::Disconnected => Self::Disconnected,
            RTCPeerConnectionState::Failed => Self::Failed,
            RTCPeerConnectionState::Closed => Self::Closed,
            _ => Self::New,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RemotePacket {
    pub ssrc: u32,
    pub sequence: u16,
    pub timestamp: u32,
    pub payload: Bytes,
}

fn rtc_err(e: impl std::fmt::Display) -> AppError {
    AppError::Other(format!("媒体连接错误：{e}"))
}

struct Handler {
    leg: Leg,
    events: mpsc::UnboundedSender<MediaEvent>,
    remote: Option<mpsc::Sender<RemotePacket>>,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for Handler {
    async fn on_connection_state_change(&self, s: RTCPeerConnectionState) {
        let _ = self.events.send(MediaEvent::State(self.leg, s.into()));
    }

    async fn on_track(&self, track: Arc<dyn TrackRemote>) {
        let sink = self.remote.clone();
        log::info!("语音收到远端音轨 {:?}", self.leg);
        tauri::async_runtime::spawn(async move {
            while let Some(ev) = track.poll().await {
                match ev {
                    TrackRemoteEvent::OnRtpPacket(p) => {
                        if p.payload.is_empty() {
                            continue;
                        }
                        if let Some(tx) = &sink {
                            let _ = tx.try_send(RemotePacket {
                                ssrc: p.header.ssrc,
                                sequence: p.header.sequence_number,
                                timestamp: p.header.timestamp,
                                payload: p.payload.clone(),
                            });
                        }
                    }
                    TrackRemoteEvent::OnEnded => {
                        log::info!("语音远端音轨结束");
                        break;
                    }
                    _ => {}
                }
            }
        });
    }
}

struct MidExt(Bytes);

impl MarshalSize for MidExt {
    fn marshal_size(&self) -> usize {
        self.0.len()
    }
}

impl Marshal for MidExt {
    fn marshal_to(&self, buf: &mut [u8]) -> rtc::shared::error::Result<usize> {
        buf[..self.0.len()].copy_from_slice(&self.0);
        Ok(self.0.len())
    }
}

fn opus_codec() -> RTCRtpCodec {
    RTCRtpCodec {
        mime_type: "audio/opus".into(),
        clock_rate: 48000,
        channels: 2,
        sdp_fmtp_line: "minptime=10;useinbandfec=1".into(),
        rtcp_feedback: vec![],
    }
}

fn media_engine(send: bool) -> AppResult<MediaEngine> {
    let mut me = MediaEngine::default();
    me.register_codec(
        RTCRtpCodecParameters {
            rtp_codec: opus_codec(),
            payload_type: OPUS_PT,
        },
        RtpCodecKind::Audio,
    )
    .map_err(rtc_err)?;
    if send {
        for uri in [MID_URI, LEVEL_URI] {
            me.register_header_extension(
                RTCRtpHeaderExtensionCapability { uri: uri.into() },
                RtpCodecKind::Audio,
                Some(RTCRtpTransceiverDirection::Sendonly),
            )
            .map_err(rtc_err)?;
        }
    }
    Ok(me)
}

async fn build_pc(
    cert: RTCCertificate,
    leg: Leg,
    events: mpsc::UnboundedSender<MediaEvent>,
    remote: Option<mpsc::Sender<RemotePacket>>,
) -> AppResult<Arc<dyn PeerConnection>> {
    let mut me = media_engine(leg == Leg::Send)?;
    let reg = register_default_interceptors(Registry::new(), &mut me).map_err(rtc_err)?;
    let mut setting = SettingEngineBuilder::new();
    if leg == Leg::Recv {
        setting = setting.with_answering_dtls_role(RTCDtlsRole::Client);
    }
    let pc = Box::pin(
        PeerConnectionBuilder::new()
            .with_configuration(
                RTCConfigurationBuilder::new()
                    .with_certificates(vec![cert])
                    .build(),
            )
            .with_media_engine(me)
            .with_setting_engine(setting.build())
            .with_interceptor_registry(reg)
            .with_handler(Arc::new(Handler {
                leg,
                events,
                remote,
            }))
            .with_udp_addrs(vec!["0.0.0.0:0"])
            .build(),
    )
    .await
    .map_err(rtc_err)?;
    Ok(Arc::new(pc))
}

#[derive(Clone)]
pub struct Uplink {
    track: Arc<TrackLocalStaticSample>,
    ssrc: u32,
    mid: Bytes,
}

impl Uplink {
    /// # Errors
    ///
    /// 推流连接关闭或写入失败时返回错误。
    pub async fn write_opus(
        &self,
        frame: Bytes,
        duration: Duration,
        level: u8,
        skipped: u16,
    ) -> AppResult<()> {
        let sample = Sample {
            data: frame,
            duration,
            prev_dropped_packets: skipped,
            ..Sample::new(Instant::now())
        };
        self.track
            .sample_writer(self.ssrc, OPUS_PT)
            .with_audio_level(AudioLevelExtension {
                level: level.min(127),
                voice: level < 127,
            })
            .with_extension(HeaderExtension::Custom {
                uri: MID_URI.into(),
                extension: Box::new(MidExt(self.mid.clone())),
            })
            .write_sample(&sample)
            .await
            .map_err(rtc_err)
    }
}

pub struct Media {
    send: Arc<dyn PeerConnection>,
    recv: Arc<dyn PeerConnection>,
    fingerprint: String,
    offer: LocalOffer,
    uplink: Uplink,
}

struct PcGuard(Vec<Arc<dyn PeerConnection>>);

impl Drop for PcGuard {
    fn drop(&mut self) {
        for pc in self.0.drain(..) {
            tauri::async_runtime::spawn(async move {
                let _ = pc.close().await;
            });
        }
    }
}

impl Media {
    /// # Errors
    ///
    /// 创建证书或连接失败时返回错误，已创建的连接会关闭。
    pub async fn new(
        events: mpsc::UnboundedSender<MediaEvent>,
        remote: Option<mpsc::Sender<RemotePacket>>,
    ) -> AppResult<Self> {
        let provider = rtc::crypto::default_provider().map_err(|e| rtc_err(format!("{e:?}")))?;
        let cert = RTCCertificate::generate(
            provider.crypto(),
            rtc::crypto::SignatureScheme::EcdsaP256Sha256,
            CertificateParams::new(vec!["cdapp".to_owned()]).map_err(rtc_err)?,
        )
        .map_err(rtc_err)?;
        let fingerprint = cert
            .get_fingerprints(provider.crypto())
            .map_err(rtc_err)?
            .into_iter()
            .find(|f| f.algorithm == "sha-256")
            .ok_or_else(|| rtc_err("证书缺少 sha-256 指纹"))?
            .value
            .to_ascii_uppercase();
        let send = Box::pin(build_pc(cert.clone(), Leg::Send, events.clone(), None)).await?;
        let mut guard = PcGuard(vec![send.clone()]);
        let recv = Box::pin(build_pc(cert, Leg::Recv, events, remote)).await?;
        guard.0.push(recv.clone());
        let media = async {
            let ssrc: u32 = rand::random();
            let track = Arc::new(
                TrackLocalStaticSample::new(
                    Instant::now(),
                    MediaStreamTrack::new(
                        "cdapp".into(),
                        "cdapp-audio".into(),
                        "audio".into(),
                        RtpCodecKind::Audio,
                        vec![RTCRtpEncodingParameters {
                            rtp_coding_parameters: RTCRtpCodingParameters {
                                ssrc: Some(ssrc),
                                ..Default::default()
                            },
                            codec: opus_codec(),
                            ..Default::default()
                        }],
                    ),
                )
                .map_err(rtc_err)?,
            );
            send.add_track(track.clone() as Arc<dyn TrackLocal>)
                .await
                .map_err(rtc_err)?;
            let offer = send.create_offer(None).await.map_err(rtc_err)?;
            send.set_local_description(offer.clone())
                .await
                .map_err(rtc_err)?;
            let parsed = LocalOffer::parse(&offer.sdp)?;
            if parsed.ssrc != ssrc {
                return Err(rtc_err("本地 offer 中的 SSRC 与音轨不一致"));
            }
            let uplink = Uplink {
                track,
                ssrc,
                mid: Bytes::from(parsed.mid.clone().into_bytes()),
            };
            Ok((parsed, uplink))
        }
        .await;
        let (offer, uplink) = media?;
        guard.0.clear();
        Ok(Self {
            send,
            recv,
            fingerprint,
            offer,
            uplink,
        })
    }

    #[must_use]
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    #[must_use]
    pub fn offer(&self) -> &LocalOffer {
        &self.offer
    }

    #[must_use]
    pub fn uplink(&self) -> Uplink {
        self.uplink.clone()
    }

    /// # Errors
    ///
    /// 应答无效时返回错误。
    pub async fn set_send_answer(&self, sdp: String) -> AppResult<()> {
        let d = RTCSessionDescription::answer(sdp).map_err(rtc_err)?;
        self.send.set_remote_description(d).await.map_err(rtc_err)
    }

    /// # Errors
    ///
    /// offer 无效时返回错误。
    pub async fn update_recv(&self, offer: String) -> AppResult<String> {
        let d = RTCSessionDescription::offer(offer).map_err(rtc_err)?;
        self.recv.set_remote_description(d).await.map_err(rtc_err)?;
        let a = self.recv.create_answer(None).await.map_err(rtc_err)?;
        let sdp = a.sdp.clone();
        self.recv.set_local_description(a).await.map_err(rtc_err)?;
        Ok(sdp)
    }

    pub async fn close(&self) {
        let _ = self.send.close().await;
        let _ = self.recv.close().await;
    }
}
