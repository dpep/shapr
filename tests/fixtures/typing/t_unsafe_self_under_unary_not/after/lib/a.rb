# typed: true
class A
  def run
    not T.unsafe(self)
  end

  private

  def -@; 1; end
  def ~; 2; end
  def !; false; end
end
