#include "INode.h"
#include "Kernel.h"

extern "C" Buf* compat_filesys_alloc(short dev) {
        return Kernel::Instance().GetFileSystem().Alloc(dev);
}

extern "C" bool compat_fs_readonly(short dev) {
        return !!Kernel::Instance().GetFileSystem().GetFS(dev)->s_ronly;
}

extern "C" void compat_filesys_free(short dev, int blkno) {
        return Kernel::Instance().GetFileSystem().Free(dev, blkno);
}

/*============================class DiskInode=================================*/

DiskInode::DiskInode()
{
	/*
	 * 如果DiskInode没有构造函数，会发生如下较难察觉的错误：
	 * DiskInode作为局部变量占据函数Stack Frame中的内存空间，但是
	 * 这段空间没有被正确初始化，仍旧保留着先前栈内容，由于并不是
	 * DiskInode所有字段都会被更新，将DiskInode写回到磁盘上时，可能
	 * 将先前栈内容一同写回，导致写回结果出现莫名其妙的数据。
	 */
	this->d_mode = 0;
	this->d_nlink = 0;
	this->d_uid = -1;
	this->d_gid = -1;
	this->d_size = 0;
	for(int i = 0; i < 10; i++)
	{
		this->d_addr[i] = 0;
	}
	this->d_atime = 0;
	this->d_mtime = 0;
}

DiskInode::~DiskInode()
{
	//nothing to do here
}

extern "C" void inode_read(Inode* pInode) {
	pInode->ReadI();
}
