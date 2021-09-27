require('dotenv').config()
const { Client } = require('pg')

const client = new Client({
    ssl: {
        rejectUnauthorized: false
    }
});

const videoId = "PaRam-aY9p0";

const youtubeClient = require("youtube-fast-api")

const ytClient = new youtubeClient(process.env.YOUTUBE_KEY_API);

const videoComments = ytClient.getAllComments(videoId);

videoComments.then(videoData => {
    saveVideoData(videoData);
})

function getUserData(videoData) {
    return {
        "authorDisplayName": videoData.authorDisplayName,
        "authorProfileImageUrl": videoData.authorProfileImageUrl,
        "authorChannelUrl": videoData.authorChannelUrl,
        "authorChannelId": videoData.authorChannelId
    }
}

function getCommentsData(videoData) {
    return {
        "videoId": videoData.videoId,
        "textDisplay": videoData.textDisplay,
        "textOriginal": videoData.textOriginal,
        "canRate": videoData.canRate,
        "viewerRating": videoData.viewerRating,
        "likeCount": videoData.likeCount,
        "publishedAt": videoData.publishedAt,
        "updatedAt": videoData.updatedAt,
        "id": videoData.id,
        "commentby": videoData.authorChannelId
    };
}

//Guarda la informacion del video en la base de datos
function saveVideoData(videoData) {
    const usersData = videoData.map(data => getUserData(data));
    saveUserInDataBase(usersData)
    const commentsData = videoData.map(data => getCommentsData(data));
    saveCommentsInDataBase(commentsData);
}

function saveUserInDataBase(usersData) {

    client.connect()
    Promise.all(usersData.map(user => saveUserInDataBase(user))).finally(x => client.end());

}

function saveCommentsInDataBase(commentsData) {

    client.connect()
    Promise.all(commentsData.map(comment => saveCommentDataBase(comment))).finally(x => client.end());

}


function saveUserInDataBase(user) {
    const insertQuery = `INSERT INTO usuarios (authordisplayname, authorprofileimageurl, authorchannelurl, authorchannelid) VALUES($1,$2,$3,$4);`;

    const values = [user.authorDisplayName, user.authorProfileImageUrl, user.authorChannelUrl, user.authorChannelId]

    return client
        .query(insertQuery, values)
        .then(res => {
            console.log("Todo ok")

        })
        .catch(e => console.error(e.stack))

}

function saveCommentDataBase(comment) {

    const insertQuery = `INSERT INTO public.comentarios (videoid, textdisplay, textoriginal, canrate, viewerrating, likecount, publishedat, updatedat, id, commentby) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10);`;

    const values = [comment.videoId,
    comment.textDisplay,
    comment.textOriginal,
    comment.canRate,
    comment.viewerRating,
    comment.likeCount,
    comment.publishedAt,
    comment.updatedAt,
    comment.id,
    comment.authorChannelId]

    return client
        .query(insertQuery, values)
        .then(res => {
            console.log("Todo ok")

        })
        .catch(e => console.error(e.stack))


    /*
    INSERT INTO public.comentarios
        (videoid, textdisplay, textoriginal, canrate, viewerrating, likecount, publishedat, updatedat, id, commentby)
    VALUES('', '', '', '', '', '', '', '', '', ''); */

}



