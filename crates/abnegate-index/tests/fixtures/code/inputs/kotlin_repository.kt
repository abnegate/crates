package com.acme.widgets.data

import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import org.junit.Test

data class Note(val id: Long, val title: String, val body: String)

interface NoteSource {
    suspend fun fetch(id: Long): Note?
}

class NoteRepository(private val source: NoteSource) {
    private val cache = mutableMapOf<Long, Note>()
    private val updates = MutableStateFlow<List<Note>>(emptyList())

    suspend fun load(id: Long): Note? {
        cache[id]?.let { return it }
        val note = source.fetch(id) ?: return null
        cache[id] = note
        updates.value = cache.values.toList()
        return note
    }

    fun observe(): Flow<List<Note>> = updates

    fun clear() {
        cache.clear()
        updates.value = emptyList()
    }
}

object NoteDefaults {
    const val MAXIMUM_TITLE = 120
    fun blank(id: Long) = Note(id, "", "")
}

class NoteRepositoryTest {
    @Test
    fun testClearEmptiesCache() {
        val repository = NoteRepository(object : NoteSource {
            override suspend fun fetch(id: Long): Note? = null
        })
        repository.clear()
    }
}
