```mermaid
flowchart TB
    subgraph Frontend["Frontend (HTTP API)"]
        Web[Web]
        Mobile[Mobile]
    end
    subgraph Backend
        subgraph Requests["Request path"]
            API[API backend]
            UserService[User service]
            LibraryService[Library service]
        end
        subgraph Streaming
            PlaybackService[Playback service]
            Transcoder[Transcoder]
            TranscodeCache[transcode cache<br/>temp HLS files]
        end
        subgraph Background["Background jobs"]
            Scheduler[Scheduler]
            MetadataService[Metadata service]
            APIManager[API manager]
        end
        subgraph Database["Database"]
            UserDB[(user db)]
            MetadataDB[(metadata db<br/>indexes)]
            SchedulerDB[(tasks)]
        end
    end
    MediaStorage[(Media storage)]
    subgraph Indexing["API Components (indexing)"]
        MovieDB[movieDB]
        MusicBrainz[MusicBrainz]
    end
    Web & Mobile --> API
    API --> UserService & LibraryService
    API -->|play| PlaybackService
    UserService --> UserDB
    LibraryService -->|read| MetadataDB
    PlaybackService -->|transcode| Transcoder
    PlaybackService -->|direct play| MediaStorage
    Transcoder -->|read media| MediaStorage
    Transcoder -->|write| TranscodeCache
    Scheduler -->|enrich| MetadataService
    Scheduler --> SchedulerDB
    Scheduler -->|scan| MediaStorage
    MetadataService -->|write| MetadataDB
    MetadataService -->|fetch| APIManager
    APIManager --> MovieDB & MusicBrainz
    TranscodeCache ~~~ MediaStorage
    TranscodeCache ~~~ MovieDB
```

**Media storage** is S3-compatible object storage (RustFS); new uploads reach the Transcoder as
events over an MQTT broker (Mosquitto). See [Infrastructure](infrastructure.md), and
[Setup](setup.md) for a development environment.
