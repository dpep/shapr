class A
  def self.make
    define_method(:go) do
      setup
    end
  end

  def self.setup
    @a = 1
    @b = 2
    @c = @a + @b
  end
end
