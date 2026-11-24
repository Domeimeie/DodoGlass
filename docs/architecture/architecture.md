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
        end
        subgraph Catalogue
            AssetServer["Asset server<br/>catalogue + metadata"]
        end
        subgraph Streaming
            PlaybackService[Playback service]
            Transcoder[Transcoder]
            TranscodeCache[transcode cache<br/>temp HLS files]
        end
        subgraph Background["Background jobs"]
            Scheduler[Scheduler]
            APIManager[API manager]
        end
        subgraph Database["Database"]
            UserDB[(user db)]
            CatalogueDB[(catalogue db<br/>SQLite)]
            SchedulerDB[(tasks)]
        end
    end
    subgraph MediaStorage["Media storage (RustFS)"]
        Ingest[(ingest)]
        Final[(final)]
        Assets[(assets<br/>posters, stills)]
    end
    Broker{{"MQTT broker<br/>(Mosquitto)"}}
    subgraph Indexing["API Components (indexing)"]
        TMDB[TMDB]
        MusicBrainz[MusicBrainz]
    end
    Web & Mobile --> API
    API --> UserService & AssetServer
    API -->|play| PlaybackService
    UserService --> UserDB
    Ingest -.->|upload events| Broker
    Final -.->|output marker events| Broker
    Broker -->|tasks| Transcoder
    Broker -->|finished outputs| AssetServer
    Transcoder -->|read| Ingest
    Transcoder -->|HLS + DASH| Final
    Transcoder -.->|status| Broker
    AssetServer -->|read / write| CatalogueDB
    AssetServer -->|reconcile| Final
    AssetServer -->|images| Assets
    AssetServer -->|match| TMDB
    PlaybackService -->|stream| Final
    PlaybackService -->|transcode on demand| TranscodeCache
    Scheduler -.->|refresh, later| AssetServer
    Scheduler --> SchedulerDB
    Scheduler -->|fetch| APIManager
    APIManager --> MusicBrainz

    classDef built fill:#d8f0d8,stroke:#2e7d32,color:#1b3d1b
    class AssetServer,Transcoder,Ingest,Final,Assets,Broker,CatalogueDB,TMDB built
```

Green parts are built; the rest is planned. The **Asset server** combines the planned Library and
Metadata services (see [Asset server](asset-server.md)).

**Media storage** is S3-compatible object storage (RustFS); new uploads reach the Transcoder, and
finished outputs (their `source.json` in `final`) the Asset server, as events over an MQTT broker
(Mosquitto). See [Infrastructure](infrastructure.md), and
[Setup](setup.md) for a development environment.
