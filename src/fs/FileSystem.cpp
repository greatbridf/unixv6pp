#include "FileSystem.h"
#include "Utility.h"
#include "New.h"
#include "Kernel.h"
#include "OpenFileManager.h"
#include "TimeInterrupt.h"
#include "Video.h"
#include "fs_defines.h"

extern "C" bool FileSystem_load_super_block();
extern "C" bool FileSystem_get_fs(short, SuperBlock*);
extern "C" bool FileSystem_is_readonly(short);
extern "C" void FileSystem_update();
extern "C" Inode* FileSystem_i_alloc(short);
extern "C" void FileSystem_i_free(short, int);
extern "C" Buf* FileSystem_alloc(short);
extern "C" void FileSystem_free(short, int);
extern "C" Mount* FileSystem_get_mount(Inode*);

/*==============================class SuperBlock===================================*/
/* 系统全局超级块SuperBlock对象 */
SuperBlock g_spb;

SuperBlock::SuperBlock()
{
	//nothing to do here
}

SuperBlock::~SuperBlock()
{
	//nothing to do here
}

/*==============================class Mount===================================*/
Mount::Mount()
{
	this->m_dev = -1;
	this->m_spb = NULL;
	this->m_inodep = NULL;
}

Mount::~Mount()
{
	this->m_dev = -1;
	this->m_inodep = NULL;
	//释放内存SuperBlock副本
	if(!this->m_spb)
                return;

        delete this->m_spb;
        this->m_spb = NULL;
}

/*==============================class FileSystem===================================*/
FileSystem::FileSystem()
{
	//nothing to do here
}

FileSystem::~FileSystem()
{
	//nothing to do here
}

void FileSystem::Initialize()
{
	//nothing to do here
}

void FileSystem::LoadSuperBlock()
{
	if (!FileSystem_load_super_block())
                Utility::Panic("Load SuperBlock Error....!\n");
}

SuperBlock* FileSystem::GetFS(short dev)
{
	if(FileSystem_get_fs(dev, &g_spb))
                return &g_spb;

	Utility::Panic("No File System!");
	return NULL;
}

bool FileSystem::IsReadOnly(short dev)
{
        return FileSystem_is_readonly(dev);
}

void FileSystem::Update()
{
	FileSystem_update();
}

Inode* FileSystem::IAlloc(short dev)
{
	return FileSystem_i_alloc(dev);
}

void FileSystem::IFree(short dev, int number)
{
	FileSystem_i_free(dev, number);
}

Buf* FileSystem::Alloc(short dev)
{
	return FileSystem_alloc(dev);
}

void FileSystem::Free(short dev, int blkno)
{
	FileSystem_free(dev, blkno);
}

Mount* FileSystem::GetMount(Inode *pInode)
{
	return FileSystem_get_mount(pInode);
}

bool FileSystem::BadBlock(SuperBlock *spb, short dev, int blkno)
{
	return 0;
}
