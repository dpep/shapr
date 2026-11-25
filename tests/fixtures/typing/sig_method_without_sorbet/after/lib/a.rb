class A
  def self.sig(&blk)
    (@sigs ||= []) << blk
  end

  sig { returns(Integer) }
  def total
    1
  end
end
