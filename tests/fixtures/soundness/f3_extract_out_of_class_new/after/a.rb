class A
  def build
    Class.new do
      setup
    end
  end

  def setup
    @a = 1
    @b = 2
    @c = @a + @b
  end
end
