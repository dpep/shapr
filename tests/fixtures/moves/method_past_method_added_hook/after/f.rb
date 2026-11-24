class A
  def x
    1
  end

  def self.method_added(n)
    puts n
  end
end
