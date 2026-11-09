//! 本地音频：设备采集与播放、重采样、抖动缓冲、混音与麦克风状态。
//! 设备回调运行在独立线程，与异步任务之间通过有界队列和通道交换数据。

pub mod capture;
pub mod denoise;
pub mod dsp;
pub mod effect;
pub mod jitter;
pub mod loopback;
pub mod mic;
pub mod mixer;
pub mod output;
pub mod store;
pub mod watch;
