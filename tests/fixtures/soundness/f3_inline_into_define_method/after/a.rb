class A
  def self.make
    define_method(:go) do
      @a = 1
      @b = 2
      @c = @a + @b
    end
  end
end
