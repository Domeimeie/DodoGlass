# DodoGlass

A self-hosted media platform: videos are uploaded, transcoded for streaming and listed in a catalogue
with their descriptive information.

## Catalogue

**Asset server**:
The service that owns the catalogue: which Movies, Shows, Seasons and Episodes exist, their
descriptive information, and which Media files belong to them.
_Avoid_: Library service, metadata service (as separate things)

**Movie**:
A single film in the catalogue, identified by its name and release year. Has one or more Media files.
_Avoid_: Film, title

**Show**:
A series in the catalogue, identified by its name and first-air year, made of Seasons. Has no Media
files of its own.
_Avoid_: Series, TV show

**Season**:
A numbered part of a Show, made of Episodes. Season 0 holds the Show's specials.

**Episode**:
A numbered part of a Season. Has one or more Media files.

**Media file**:
One uploaded video, transcoded for streaming and belonging to a Movie or an Episode. Identified by
its full upload path: uploading to the same path again is a new version of the same Media file.
_Avoid_: Asset, file, video (when the catalogue entry is meant)

**Label**:
The short name that tells the Media files of one Movie or Episode apart, taken from the upload's file
name (e.g. "matrix-directors-cut").
_Avoid_: Edition, version

**Default media file**:
The Media file of a Movie or Episode that plays unless the viewer picks another one.
_Avoid_: Main file, primary

**Unassigned media file**:
A Media file whose upload path does not follow the upload layout (missing year, unknown folder,
mismatching season), waiting for an admin to attach it to a Movie or Episode.

**Match**:
The link between a Movie, Show, Season or Episode and its entry in TMDB, from which its descriptive
information is filled in.

**Needs review**:
The state of a catalogue entry for which no Match could be made with certainty, waiting for an admin to
pick one.
_Avoid_: Unmatched, pending

**Rating**:
TMDB's user score of a Movie, Show or Episode (0–10) with its number of votes. Never a Locked field:
it is refreshed periodically, while all other descriptive information is fetched once.
_Avoid_: Score, vote, age rating (an age rating is a certification, not a Rating)

**Locked field**:
A descriptive field an admin has edited by hand; it is never overwritten by a new Match or a refresh.

### Media file states

**Processing**:
A Media file that has been uploaded and whose transcoded output is not complete yet.
_Avoid_: Pending, transcoding

**Ready**:
A Media file whose transcoded output is complete and can be played.
_Avoid_: Done, finished, available

**Failed**:
A Media file whose transcode did not complete; uploading to the same path again retries it.

**Missing**:
A Media file the catalogue knows as Ready whose transcoded output has disappeared from storage.
_Avoid_: Deleted, removed

### Visibility

**Hidden**:
A Movie, Show, Season or Episode kept in the catalogue, with its descriptive information and Locked
fields, but not shown to viewers because none of its Media files is Ready.
_Avoid_: Deleted, archived
