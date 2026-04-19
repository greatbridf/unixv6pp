#ifndef FILE_SYSTEM_H
#define FILE_SYSTEM_H

#include "INode.h"
#include "Buf.h"
#include "BufferManager.h"

#include <fs_defines.h>

/*
 * 文件系统存储资源管理块(Super Block)的定义。
 */
class SuperBlock
{
	/* Functions */
public:
	/* Constructors */
	SuperBlock();
	/* Destructors */
	~SuperBlock();

	/* Members */
public:
	int		s_isize;		/* 外存Inode区占用的盘块数 */
	int		s_fsize;		/* 盘块总数 */

	int		s_nfree;		/* 直接管理的空闲盘块数量 */
	int		s_free[100];	/* 直接管理的空闲盘块索引表 */

	int		s_ninode;		/* 直接管理的空闲外存Inode数量 */
	int		s_inode[100];	/* 直接管理的空闲外存Inode索引表 */

	int		s_flock;		/* 封锁空闲盘块索引表标志 */
	int		s_ilock;		/* 封锁空闲Inode表标志 */

	int		s_fmod;			/* 内存中super block副本被修改标志，意味着需要更新外存对应的Super Block */
	int		s_ronly;		/* 本文件系统只能读出 */
	int		s_time;			/* 最近一次更新时间 */
	int		padding[47];	/* 填充使SuperBlock块大小等于1024字节，占据2个扇区 */
};


/*
 * 文件系统装配块(Mount)的定义。
 * 装配块用于实现子文件系统与
 * 根文件系统的连接。
 */
class Mount
{
	/* Functions */
public:
	/* Constructors */
	Mount();
	/* Destructors */
	~Mount();

	/* Members */
public:
	short 		m_dev;		/* 文件系统设备号 */
	SuperBlock* m_spb;		/* 指向文件系统的Super Block对象在内存中的副本 */
	Inode*		m_inodep;	/* 指向挂载子文件系统的内存INode */
};



/*
 * 文件系统类(FileSystem)管理文件存储设备中
 * 的各类存储资源，磁盘块、外存INode的分配、
 * 释放。
 */
class FileSystem
{
public:
	/* static consts */
	static const int NMOUNT = 5;			/* 系统中用于挂载子文件系统的装配块数量 */
	static const int ROOTINO = 1;			/* 文件系统根目录外存Inode编号 */

	static const unsigned long INODE_SIZE = 64;
	static const unsigned long INODE_NUMBER_PER_SECTOR = fs::SECTOR_SIZE / INODE_SIZE;

	/* Functions */
public:
	/* Constructors */
	FileSystem();
	/* Destructors */
	~FileSystem();

	/*
	 * @comment 初始化成员变量
	 */
	void Initialize();

	/*
	* @comment 系统初始化时读入SuperBlock
	*/
	void LoadSuperBlock();

	/*
	 * @comment 根据文件存储设备的设备号dev获取
	 * 该文件系统的SuperBlock
	 */
	SuperBlock* GetFS(short dev);
	/*
	 * @comment 检查文件系统是否只读
	 */
	bool IsReadOnly(short dev);
	/*
	 * @comment 将SuperBlock对象的内存副本更新到
	 * 存储设备的SuperBlock中去
	 */
	void Update();

	/*
	 * @comment  在存储设备dev上分配一个空闲
	 * 外存INode，一般用于创建新的文件。
	 */
	Inode* IAlloc(short dev);
	/*
	 * @comment  释放存储设备dev上编号为number
	 * 的外存INode，一般用于删除文件。
	 */
	void IFree(short dev, int number);

	/*
	 * @comment 在存储设备dev上分配空闲磁盘块
	 */
	Buf* Alloc(short dev);
	/*
	 * @comment 释放存储设备dev上编号为blkno的磁盘块
	 */
	void Free(short dev, int blkno);

	/*
	 * @comment 查找文件系统装配表，搜索指定Inode对应的Mount装配块
	 */
	Mount* GetMount(Inode* pInode);

private:
	/*
	 * @comment 检查设备dev上编号blkno的磁盘块是否属于
	 * 数据盘块区
	 */
	bool BadBlock(SuperBlock* spb, short dev, int blkno);

private:
	// no members
};

#endif
